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

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/receiver-trace")
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
    let tmp = tempfile::tempdir().expect("temp dir");
    let repo = tmp.path().join("workspace");
    copy_dir(&fixture_dir().join("repo"), &repo);
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    let cassettes = tmp.path().join("cassettes");
    std::fs::create_dir_all(cassettes.join("library-claims")).unwrap();
    std::fs::copy(
        fixture_dir().join("library-claims.json"),
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
