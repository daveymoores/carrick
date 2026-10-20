//! carrick#2447: a request URL built from parts whose values the source
//! states is read as one value, however the parts are written and in
//! whichever module they are declared. A part with no stated value stays an
//! unread value.
//!
//! Deterministic end to end. The LLM is replayed from `__llm__/`, and every
//! cassette is empty (the only candidate each file raises is the wrapper's
//! own `fetch(url)`), so every row asserted here is read off the source by
//! the request summaries.
//!
//! See `tests/fixtures/composed-strings/README.md` for the shape and the
//! answer key.

use std::process::Command;
use std::sync::OnceLock;

fn calls() -> &'static [serde_json::Value] {
    static CALLS: OnceLock<Vec<serde_json::Value>> = OnceLock::new();
    CALLS.get_or_init(|| {
        let repo = env!("CARGO_MANIFEST_DIR");
        let fixture = format!("{repo}/tests/fixtures/composed-strings");
        let mock_dir = format!("{fixture}/__llm__/");

        let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
            .arg(&fixture)
            .env("CARRICK_MOCK_ALL", "1")
            .env("CARRICK_MOCK_FIXTURE_DIR", &mock_dir)
            .env("CARRICK_OUTPUT_JSON", "1")
            .env("CARRICK_SKIP_INTENTS", "1")
            .env_remove("GITHUB_REPOSITORY")
            .env_remove("GITHUB_ACTIONS")
            .env_remove("CI")
            .output()
            .expect("failed to spawn carrick binary");

        assert!(
            output.status.success(),
            "scanner exited non-zero:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("scanner stdout was not UTF-8");
        let projection: serde_json::Value =
            serde_json::from_str(&stdout).expect("scanner output was not valid JSON");
        projection["calls"]
            .as_array()
            .expect("projection carries a calls array")
            .clone()
    })
}

/// No call at `file:line`, from any source: the URL there reads as more than
/// one written path, so no one path is its (carrick#2541).
fn assert_no_row(file: &str, line: i64) {
    let rows: Vec<_> = calls()
        .iter()
        .filter(|call| call["file"].as_str() == Some(file) && call["line"].as_i64() == Some(line))
        .collect();
    assert!(
        rows.is_empty(),
        "expected no row at {file}:{line}, got {rows:#?}"
    );
}

/// The one row at `file:line`, stated by the summaries, sending `GET` to
/// `target`.
fn assert_row(file: &str, line: i64, target: &str) {
    let rows: Vec<_> = calls()
        .iter()
        .filter(|call| call["file"].as_str() == Some(file) && call["line"].as_i64() == Some(line))
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "expected one row at {file}:{line} ({target}), got {rows:#?}"
    );
    let row = rows[0];
    assert_eq!(
        row["resolution_source"].as_str(),
        Some("request_summary"),
        "{file}:{line} is read off the source: {row:#}"
    );
    assert_eq!(
        row["method"].as_str(),
        Some("GET"),
        "{file}:{line}: {row:#}"
    );
    assert_eq!(
        row["target_url"].as_str(),
        Some(target),
        "{file}:{line}: {row:#}"
    );
}

#[test]
fn a_base_an_imported_builder_returns_is_composed_into_the_url() {
    let file = "src/client.ts";
    // A builder that returns text.
    assert_row(file, 17, "/api/v1/session");
    // The same builder written with a local before its `return`.
    assert_row(file, 23, "/api/v1/local");
    // A builder handed text.
    assert_row(file, 50, "/items/abc/text");
}

#[test]
fn a_constant_prefix_and_a_constant_object_member_are_composed_into_the_url() {
    let file = "src/client.ts";
    assert_row(file, 20, "/api/v1/x/${id}");
    assert_row(file, 26, "/users/m");
}

#[test]
fn a_function_whose_every_return_is_one_value_is_composed() {
    assert_row("src/client.ts", 32, "/a/same");
    assert_row("src/local.ts", 30, "/a/same");
}

#[test]
fn builders_declared_beside_the_client_compose_too() {
    let file = "src/local.ts";
    assert_row(file, 21, "/api/v1/session");
    assert_row(file, 24, "/api/v1/local");
}

#[test]
fn a_call_that_reads_as_several_paths_states_no_row() {
    // Two different returns, each a path: `/a` or `/b`, so no one path.
    assert_no_row("src/client.ts", 29);
    assert_no_row("src/local.ts", 27);
}

#[test]
fn a_part_with_no_stated_value_stays_an_unread_value() {
    let file = "src/client.ts";
    // A runtime value.
    assert_row(file, 35, "${process.env.X}/env");
    // An object another function writes to.
    assert_row(file, 41, "${MUTABLE.users}/mutable");
    // A builder that is not only constants and one return.
    assert_row(file, 44, "${viaLet()}/let");
    assert_row(file, 47, "${afterLoop([])}/loop");
    // A builder handed a value no module states.
    assert_row(file, 53, "${itemPath(id)}/param");
}
