//! The receiver trace's switch (carrick#2474 R1).
//!
//! Drives the real scanner binary on `.`, as the Action runs it, over a
//! committed copy of `tests/fixtures/receiver-trace/repo` (one root service),
//! offline, with the fixture's store answer replayed as the library store's.
//! With the fact layer on (`CARRICK_POSITION_ROWS=1`,
//! `CARRICK_SITE_QUESTIONS=1`) and `CARRICK_RECEIVER_TRACE` unset, the routes
//! only the trace finds leave nothing in the blob; with it set they reach
//! the blob as the site pass writes route sites. With the fact layer off the
//! switch changes no byte.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "carrick-test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "carrick-test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        // One commit hash for every copy, so two scans' blobs compare.
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

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

/// Every blob the scan stored, by file name, with the run's own stamps
/// removed.
type Blobs = BTreeMap<String, String>;

/// One offline scan of a committed copy of the fixture: the fact layer on or
/// off, and the trace switch set to `trace` or unset.
fn scan(fact_layer: bool, trace: Option<&str>) -> Blobs {
    scan_fixture("receiver-trace", fact_layer, trace)
}

/// [`scan`] of the fixture `name`.
fn scan_fixture(name: &str, fact_layer: bool, trace: Option<&str>) -> Blobs {
    let tmp = tempfile::tempdir().expect("temp dir");
    let repo = tmp.path().join("workspace");
    copy_dir(&fixture_dir(name).join("repo"), &repo);
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    let cassettes = tmp.path().join("cassettes");
    std::fs::create_dir_all(cassettes.join("library-claims")).unwrap();
    std::fs::copy(
        fixture_dir(name).join("library-claims.json"),
        cassettes.join("library-claims/default.json"),
    )
    .unwrap();
    let storage = tmp.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_carrick"));
    cmd.arg(".")
        .current_dir(&repo)
        .env("CARRICK_LOCAL_STORAGE_DIR", &storage)
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_CACHE_DIR", tmp.path().join("cache"))
        .env("HOME", &home)
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", cassettes.display()),
        )
        .env("CARRICK_SKIP_INTENTS", "1");
    for var in [
        "CARRICK_POSITION_ROWS",
        "CARRICK_SITE_QUESTIONS",
        "CARRICK_RECEIVER_TRACE",
        "GITHUB_REPOSITORY",
        "GITHUB_REF",
        "GITHUB_EVENT_NAME",
        "GITHUB_EVENT_PATH",
        "GITHUB_SHA",
        "GITHUB_RUN_ID",
        "GITHUB_ACTIONS",
        "GITHUB_WORKSPACE",
        "CI",
        "CARRICK_OUTPUT_JSON",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    ] {
        cmd.env_remove(var);
    }
    if fact_layer {
        cmd.env("CARRICK_POSITION_ROWS", "1")
            .env("CARRICK_SITE_QUESTIONS", "1");
    }
    if let Some(trace) = trace {
        cmd.env("CARRICK_RECEIVER_TRACE", trace);
    }
    let output = cmd.output().expect("spawn carrick");
    assert!(
        output.status.success(),
        "scan exited non-zero:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut blobs = Blobs::new();
    for entry in std::fs::read_dir(&storage).expect("the store reads") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let mut blob: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if let Some(object) = blob.as_object_mut() {
            for stamp in ["scanner_version", "scanner_build", "last_updated"] {
                object.remove(stamp);
            }
        }
        blobs.insert(
            path.file_name().unwrap().to_string_lossy().into_owned(),
            serde_json::to_string(&blob).unwrap(),
        );
    }
    assert!(!blobs.is_empty(), "the scan stored a blob");
    blobs
}

/// Every possible call the blobs hold, as `file:line member`.
fn possible_calls(blobs: &Blobs) -> Vec<String> {
    let mut out: Vec<String> = blobs
        .values()
        .flat_map(|text| {
            let blob: serde_json::Value = serde_json::from_str(text).unwrap();
            blob["possible_calls"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|call| {
            format!(
                "{}:{} {}",
                call["file_path"].as_str().unwrap_or_default(),
                call["line_number"],
                call["labelled_end"]["member"].as_str().unwrap_or_default()
            )
        })
        .collect();
    out.sort();
    out
}

/// carrick#2474 R1: the routes only the trace finds (a parameter, a
/// callback's parameter, a field set from a constructor parameter) reach
/// the blob as route sites with no owner only with the switch set to `1`.
/// Unset, the blob holds the route sites found without it. With the fact
/// layer off the switch changes no byte.
#[test]
fn the_receiver_trace_switch_moves_only_what_it_finds() {
    let off = scan(true, None);
    assert_eq!(possible_calls(&off), vec!["src/app.ts:25 get".to_string()]);
    let on = scan(true, Some("1"));
    assert_eq!(
        possible_calls(&on),
        vec![
            "src/app.ts:19 post".to_string(),
            "src/app.ts:25 get".to_string(),
            // The file's other routes have endpoint rows at their lines.
            "src/helpers.ts:47 get".to_string(),
            "src/helpers.ts:61 get".to_string(),
            "src/routes.ts:11 get".to_string(),
            "src/users.ts:8 get".to_string(),
        ]
    );
    // Only `1` turns it on.
    assert_eq!(scan(true, Some("true")), off);
    // With the fact layer off the switch changes no byte.
    assert_eq!(scan(false, Some("1")), scan(false, None));
}

/// carrick#2474 R2b: the scope and mount edges and the composed paths are
/// record-only. On a fixture of mounts and plugin scopes the switch moves
/// only the route sites R1's declaration arm finds (a plugin's scoped app,
/// a scope a member returns, an option's callback), and with the fact layer
/// off it changes no byte.
#[test]
fn the_scope_trace_writes_nothing_of_its_own() {
    let off = scan_fixture("receiver-scopes", true, None);
    let mounted = [
        "src/admin.ts:8 get",
        "src/extra.ts:9 post",
        "src/reports.ts:9 get",
        "src/shared.ts:8 get",
        "src/users.ts:8 get",
        "src/users.ts:9 post",
    ];
    assert_eq!(possible_calls(&off), mounted.to_vec());
    let on = scan_fixture("receiver-scopes", true, Some("1"));
    let mut traced: Vec<&str> = mounted.to_vec();
    traced.extend([
        "src/index.ts:26 get",
        "src/index.ts:37 get",
        "src/index.ts:38 get",
        "src/options.ts:16 get",
        "src/versioned.ts:10 get",
        "src/versioned.ts:22 get",
        "src/versioned.ts:28 get",
    ]);
    traced.sort();
    assert_eq!(possible_calls(&on), traced);
    assert_eq!(
        scan_fixture("receiver-scopes", false, Some("1")),
        scan_fixture("receiver-scopes", false, None)
    );
}

/// Why each possible call's path is unread, as `file:line` to reason. A call
/// whose path is read and whose owner is not is `owner_unknown`.
fn unread_reasons(blobs: &Blobs) -> BTreeMap<String, String> {
    blobs
        .values()
        .flat_map(|text| {
            let blob: serde_json::Value = serde_json::from_str(text).unwrap();
            blob["possible_calls"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|call| {
            (
                format!(
                    "{}:{}",
                    call["file_path"].as_str().unwrap_or_default(),
                    call["line_number"]
                ),
                call["unread"]["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// carrick#2662: a registration with a spread argument is a route site like
/// any other once the receiver trace is on. The arguments before the spread
/// read as written (the path is read, so only the owner is unknown, as for a
/// registration with none), and a slot at or after it reads as unread. The
/// registrations are on a local instance, on a parameter, and on a router
/// whose members no declaration verifies for, so only the instance the
/// maker builds names the op. Without the trace they stay no site.
#[test]
fn a_spread_argument_leaves_the_slots_before_it_read() {
    let on = unread_reasons(&scan_fixture("receiver-spread", true, Some("1")));
    let off = unread_reasons(&scan_fixture("receiver-spread", true, None));
    // The registrations with no spread, on the instances the maker builds.
    for line in ["src/app.ts:20", "src/app.ts:29"] {
        assert_eq!(on.get(line).map(String::as_str), Some("owner_unknown"));
        assert_eq!(off.get(line).map(String::as_str), Some("owner_unknown"));
    }
    for line in [
        // A spread of a call after a handler; a spread at the handler slot.
        "src/app.ts:21",
        "src/app.ts:22",
        // The same on the router whose declarations verify nothing.
        "src/app.ts:30",
        "src/app.ts:31",
        // A parameter.
        "src/spread.ts:15",
        "src/spread.ts:16",
    ] {
        assert_eq!(
            on.get(line).map(String::as_str),
            Some("owner_unknown"),
            "{line}: the path is read and only the owner is unknown"
        );
        assert!(!off.contains_key(line), "{line}: no site without the trace");
    }
    // With the fact layer off the switch changes no byte.
    assert_eq!(
        scan_fixture("receiver-spread", false, Some("1")),
        scan_fixture("receiver-spread", false, None)
    );
}
