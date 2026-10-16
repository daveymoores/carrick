//! Routes a host callback registers on a plugin's scope take the scope's
//! prefix (carrick#2316, slice 2a).
//!
//! The fixture is a workspace of two packages. `@fx/routes` registers its
//! routes inside an inline plugin with a prefix and hands the plugin's
//! instance to a callback its options name (`opts.registerExtra(api)`).
//! `@fx/server` passes the plugin that callback, which calls back into
//! `@fx/routes` with the instance. The `routes` service's own route
//! descriptors document those routes under the prefix.
//!
//! The scan is the binary run on `.` with the model's answers replayed from
//! the fixture's cassette, the way the Action runs it. What moves is what the
//! deterministic layer does with the rows the model states.

use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE: &str = "registration-scope-callback";

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// The one scan every test reads.
fn scan() -> &'static serde_json::Value {
    static SCAN: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    SCAN.get_or_init(run_scan)
}

fn run_scan() -> serde_json::Value {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(FIXTURE);
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("workspace");
    copy_dir(&fixture, &repo);
    let cassette = repo.join("__llm__");

    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .arg(".")
        .current_dir(&repo)
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", cassette.display()),
        )
        .env("CARRICK_OUTPUT_JSON", "1")
        .env("CARRICK_SKIP_INTENTS", "1")
        .env_remove("GITHUB_REPOSITORY")
        .env_remove("GITHUB_ACTIONS")
        .env_remove("GITHUB_EVENT_NAME")
        .env_remove("GITHUB_REF")
        .env_remove("CI")
        .output()
        .expect("failed to spawn carrick binary");
    assert!(
        output.status.success(),
        "scanner exited non-zero:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("scanner stdout was not UTF-8");
    serde_json::from_str(&stdout).expect("scanner output was not valid JSON")
}

/// Every endpoint row whose file ends with `file`, as `(path, resolution
/// source)`, sorted.
fn rows_in(projection: &serde_json::Value, file: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = projection["endpoints"]
        .as_array()
        .expect("the projection carries an endpoints array")
        .iter()
        .filter(|row| row["file"].as_str().is_some_and(|f| f.ends_with(file)))
        .map(|row| {
            (
                row["path"].as_str().unwrap_or_default().to_string(),
                row["resolution_source"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}

fn paths_in(projection: &serde_json::Value, file: &str) -> Vec<String> {
    rows_in(projection, file)
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

/// Every endpoint row, for a failure message.
fn all_rows(projection: &serde_json::Value) -> String {
    projection["endpoints"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            format!(
                "{} {} {}",
                row["file"], row["path"], row["resolution_source"]
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_route_the_host_callback_registers_takes_the_scope_prefix_and_folds() {
    let projection = scan();
    let all = all_rows(projection);
    let notes = rows_in(projection, "packages/routes/src/notes.ts");
    assert_eq!(notes.len(), 1, "one row for the note route:\n{all}");
    assert_eq!(notes[0].0, "/api/v1/teams/:id/notes", "{all}");
    assert_eq!(
        notes[0].1, "descriptor_route",
        "the documentation row folds onto the handler row:\n{all}"
    );
    let tags = rows_in(projection, "packages/routes/src/tags.ts");
    assert_eq!(
        tags.iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        vec!["/api/v1/teams/:id/tags"],
        "a second function reached from the same hand-out:\n{all}"
    );
    let documented: Vec<String> = paths_in(projection, "packages/routes/src/docs.ts");
    assert_eq!(
        documented,
        vec!["/api/v1/teams/{id}/audit".to_string()],
        "only the route written in the host package stays a documentation row:\n{all}"
    );
}

#[test]
fn the_scope_keeps_the_routes_the_model_stated() {
    let projection = scan();
    assert_eq!(
        paths_in(projection, "packages/routes/src/orders.ts"),
        vec!["/api/v1/orders"],
        "{}",
        all_rows(projection)
    );
}

#[test]
fn a_function_sharing_its_file_with_other_routes_is_not_mounted() {
    let projection = scan();
    assert_eq!(
        paths_in(projection, "packages/routes/src/shared.ts"),
        vec!["/admin/keys", "/public/keys"],
        "{}",
        all_rows(projection)
    );
}

#[test]
fn a_file_another_mount_already_resolves_to_keeps_that_mount() {
    let projection = scan();
    assert_eq!(
        paths_in(projection, "packages/routes/src/legacy.ts"),
        vec!["/legacy/teams/:id/legacy"],
        "a second binding to the file would tie and drop the prefix it has:\n{}",
        all_rows(projection)
    );
}

#[test]
fn a_hand_out_of_the_plugins_own_instance_is_not_followed() {
    let projection = scan();
    assert_eq!(
        paths_in(projection, "packages/routes/src/root-routes.ts"),
        vec!["/teams/:id/root"],
        "{}",
        all_rows(projection)
    );
}

#[test]
fn a_callback_that_is_not_one_supplier_keeps_its_routes_paths() {
    let projection = scan();
    let all = all_rows(projection);
    for (file, path, why) in [
        (
            "twice-routes.ts",
            "/teams/:id/twice",
            "supplied by two calls",
        ),
        (
            "computed-routes.ts",
            "/teams/:id/computed",
            "a computed key follows the written one",
        ),
        (
            "built-routes.ts",
            "/teams/:id/built",
            "another call builds its options elsewhere",
        ),
        (
            "spread-routes.ts",
            "/teams/:id/spread",
            "a spread follows the written key",
        ),
    ] {
        assert_eq!(
            paths_in(projection, &format!("packages/routes/src/{file}")),
            vec![path.to_string()],
            "{why}:\n{all}"
        );
    }
}

#[test]
fn the_host_packages_own_rows_do_not_move() {
    let projection = scan();
    assert_eq!(
        paths_in(projection, "packages/server/src/extra.ts"),
        vec!["/teams/:id/audit"],
        "{}",
        all_rows(projection)
    );
}
