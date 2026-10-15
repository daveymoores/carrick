//! A call proven to send nothing states no call row, whoever stated it
//! (carrick#2281).
//!
//! Drives the real scanner binary on `.`, as the Action runs it, over a
//! committed copy of `tests/fixtures/named-dispatch/`: one service where
//! functions store a handler under a name in a `new Map()`, or bind a name
//! into a callable that sends later, and other files call them at module
//! level and inside functions. The model's answers are replayed from the
//! fixture's cassette and state a request at each such call. A line under
//! test raises no candidate unless stated, so its row carries an id that
//! joins nothing and has no span: it is placed by its line.
//!
//! Asserts on the uploaded blob.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const HANDLERS: &str = "src/handlers.ts";
const BOUND: &str = "src/bound.ts";
const USE: &str = "src/use.ts";

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/named-dispatch")
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "carrick-test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "carrick-test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
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

/// Scan a committed copy of the fixture offline, on `.`, once for the whole
/// binary, and read the one blob it uploads.
fn blob() -> &'static serde_json::Value {
    static BLOB: OnceLock<serde_json::Value> = OnceLock::new();
    BLOB.get_or_init(|| {
        let tmp = tempfile::tempdir().expect("temp dir");
        let repo = tmp.path().join("workspace");
        copy_dir(&fixture_dir(), &repo);
        run_git(&repo, &["init", "-q"]);
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        let storage = tmp.path().join("storage");
        std::fs::create_dir_all(&storage).unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        let mut cmd = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_carrick")));
        cmd.arg(".")
            .current_dir(&repo)
            .env("CARRICK_LOCAL_STORAGE_DIR", &storage)
            .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
            .env("CARRICK_CACHE_DIR", tmp.path().join("cache"))
            .env("HOME", &home)
            .env("CARRICK_MOCK_ALL", "1")
            .env(
                "CARRICK_MOCK_FIXTURE_DIR",
                format!("{}/", repo.join("__llm__").display()),
            )
            .env("CARRICK_SKIP_INTENTS", "1");
        for var in [
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
        let output = cmd.output().expect("failed to spawn carrick");
        assert!(
            output.status.success(),
            "fixture scan exited non-zero:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let blobs: Vec<serde_json::Value> = std::fs::read_dir(&storage)
            .expect("storage dir")
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| serde_json::from_slice(&std::fs::read(path).ok()?).ok())
            .filter(|blob: &serde_json::Value| blob["mount_graph"].is_object())
            .collect();
        assert_eq!(blobs.len(), 1, "one service uploads one blob");
        blobs.into_iter().next().unwrap()
    })
}

/// The (method, target, source) of every call row at `file:line`.
fn rows_at(file: &str, line: u32) -> Vec<(String, String, String)> {
    let site = format!("{file}:{line}");
    let text = |value: &serde_json::Value| value.as_str().unwrap_or_default().to_string();
    blob()["mount_graph"]["data_calls"]
        .as_array()
        .expect("the blob holds a mount graph")
        .iter()
        .filter(|call| call["file_location"] == site.as_str())
        .map(|call| {
            (
                text(&call["method"]),
                text(&call["target_url"]),
                text(&call["resolution_source"]),
            )
        })
        .collect()
}

fn as_stated(target: &str) -> Vec<(String, String, String)> {
    vec![("GET".to_string(), target.to_string(), "model".to_string())]
}

/// A call of a function that only stores a handler under a name in a
/// `new Map()` the module builds and never replaces sends nothing, at module
/// level, whichever order it takes the handler and the name in, and on the
/// opening line of a call written over three lines.
#[test]
fn a_registration_into_a_language_collection_states_no_row() {
    for line in [16, 17, 18] {
        assert_eq!(rows_at(HANDLERS, line), vec![], "{HANDLERS}:{line}");
    }
}

/// A call of a function that binds a name into a callable it hands back,
/// which sends later through a client the scan does not read, sends nothing
/// where it is written.
#[test]
fn a_name_bound_into_a_handed_out_callable_states_no_row() {
    assert_eq!(rows_at(BOUND, 7), vec![]);
}

/// Inside a function, an awaited call with a string first argument raises a
/// candidate, so its row joins it and carries a span. It goes because the
/// `table.set` its callee makes is a call on a language collection.
#[test]
fn a_span_carrying_row_at_a_registration_goes_too() {
    assert_eq!(rows_at(USE, 8), vec![]);
}

/// What the placement cannot vouch for keeps the model's row: a row on an
/// inner line of a multi-line call, one whose text writes an argument's call,
/// one at a chain whose outermost call is not the registration.
#[test]
fn a_row_the_placement_cannot_vouch_for_stays() {
    assert_eq!(rows_at(HANDLERS, 20), as_stated("/orders.save.inner"));
    assert_eq!(rows_at(HANDLERS, 22), as_stated("/orders.audit"));
    assert_eq!(rows_at(HANDLERS, 23), as_stated("/orders.later"));
}

/// A collection the module cannot vouch for is not one: a binding assigned
/// again, a file that declares its own `Map`, a file that imports one. Nor is
/// a call nothing resolves silent.
#[test]
fn a_call_not_proven_silent_keeps_its_row() {
    assert_eq!(rows_at(HANDLERS, 24), as_stated("/orders.swap"));
    assert_eq!(rows_at(HANDLERS, 25), as_stated("/orders.own"));
    assert_eq!(rows_at(HANDLERS, 26), as_stated("/orders.foreign"));
    assert_eq!(rows_at(HANDLERS, 27), as_stated("/orders.mount"));
    assert_eq!(
        rows_at(USE, 12).len(),
        1,
        "the dispatcher calls the handler"
    );
}

/// A factory that hands out a callable that may send is not silent at a call
/// that writes a route into it, at module level or in a body; nor is one that
/// sends in its own body.
#[test]
fn a_route_written_into_a_handed_out_callable_keeps_its_row() {
    assert_eq!(rows_at(BOUND, 8), as_stated("/orders/:id"));
    assert_eq!(rows_at(USE, 16), as_stated("/orders/:id/lines"));
    assert_eq!(rows_at(BOUND, 9), as_stated("/orders.warm"));
}
