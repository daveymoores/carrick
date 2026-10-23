//! The dispatch routes in a scan (carrick#2474 R4a).
//!
//! Drives the real scanner binary on `.`, as the Action runs it, over a
//! committed copy of `tests/fixtures/dispatch-routes/repo` (one root
//! service), offline, with the fixture's store answer replayed as the
//! library store's, and the fact layer on (`CARRICK_POSITION_ROWS=1`,
//! `CARRICK_SITE_QUESTIONS=1`). With `CARRICK_RECEIVER_TRACE=1` the run log
//! holds one `receiver trace: dispatch route` line per route the answer key
//! states, and one `receiver trace: handler records` summary (carrick#2474
//! R4b) with the records key's counts; unset, it holds neither. Neither
//! reaches anything else: the blobs are the same with the switch and
//! without it.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dispatch-routes")
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

/// One scan: its blobs (by file name, the run's own stamps removed) and the
/// dispatch-route lines of its run log, each as `file:line METHOD path
/// evidence`, sorted.
struct Scan {
    blobs: Vec<(String, String)>,
    routes: Vec<String>,
    /// The handler-records summary lines (carrick#2474 R4b), each as
    /// `records=… asked=… forced=…`.
    records: Vec<String>,
}

/// One offline scan of a committed copy of the fixture with the fact layer
/// on, the trace switch set to `trace` or unset.
fn scan(trace: Option<&str>) -> Scan {
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
        .env("CARRICK_SKIP_INTENTS", "1")
        .env("CARRICK_POSITION_ROWS", "1")
        .env("CARRICK_SITE_QUESTIONS", "1");
    for var in [
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
    let mut blobs = Vec::new();
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
        blobs.push((
            path.file_name().unwrap().to_string_lossy().into_owned(),
            serde_json::to_string(&blob).unwrap(),
        ));
    }
    blobs.sort();
    assert!(!blobs.is_empty(), "the scan stored a blob");
    let logs: Vec<String> = std::fs::read_dir(home.join(".carrick/logs/runs"))
        .expect("the scan keeps a run log")
        .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path()).ok())
        .collect();
    let records: Vec<String> = logs
        .iter()
        .flat_map(|log| log.lines())
        .filter_map(|line| line.split_once("receiver trace: handler records "))
        .map(|(_, fields)| {
            let field = |name: &str| {
                fields
                    .split(' ')
                    .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
                    .unwrap_or_else(|| panic!("no {name} in {fields}"))
                    .to_string()
            };
            format!(
                "records={} asked={} forced={}",
                field("records"),
                field("asked"),
                field("forced")
            )
        })
        .collect();
    let mut routes: Vec<String> = logs
        .iter()
        .flat_map(|log| {
            log.lines()
                .filter_map(|line| line.split_once("receiver trace: dispatch route "))
                .map(|(_, fields)| {
                    // `file=… line=… method=… path=… evidence=…`, each value
                    // written bare.
                    let field = |name: &str| {
                        fields
                            .split(' ')
                            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
                            .unwrap_or_else(|| panic!("no {name} in {fields}"))
                            .to_string()
                    };
                    format!(
                        "{}:{} {} {} {}",
                        field("file"),
                        field("line"),
                        field("method"),
                        field("path"),
                        field("evidence")
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    routes.sort();
    Scan {
        blobs,
        routes,
        records,
    }
}

/// The records key's counts, as the run log's summary line names them.
fn key_records() -> Vec<String> {
    let key: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_dir().join("records.expected.json")).unwrap(),
    )
    .unwrap();
    let read = |what: &str| key["counts"][what].as_u64().unwrap_or_default();
    vec![format!(
        "records={} asked={} forced={}",
        read("records"),
        read("asked"),
        read("forced")
    )]
}

/// The answer key's routes, as the run log names them.
fn key_routes() -> Vec<String> {
    let key: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_dir().join("dispatch.expected.json")).unwrap(),
    )
    .unwrap();
    let mut routes: Vec<String> = key["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|route| {
            format!(
                "{}:{} {} {} {}",
                route["file"].as_str().unwrap(),
                route["line"],
                route["method"].as_str().unwrap_or("-"),
                route["path"].as_str().unwrap(),
                route["evidence"]["kind"].as_str().unwrap()
            )
        })
        .collect();
    routes.sort();
    routes
}

/// carrick#2474 R4a: with the switch set, a scan logs each route the key
/// states, the same in two scans; unset, none. The routes reach no blob.
#[test]
fn a_scan_with_the_receiver_trace_logs_each_dispatch_route() {
    let on = scan(Some("1"));
    assert_eq!(on.routes, key_routes());
    let again = scan(Some("1"));
    assert_eq!(again.routes, on.routes, "two scans log the same routes");
    assert_eq!(again.blobs, on.blobs, "two scans store the same blobs");
    let off = scan(None);
    assert!(off.routes.is_empty(), "unset, no route is logged");
    // carrick#2474 R4b: the handler records are counted in one summary line
    // per service, the key's counts, and reach no blob.
    assert_eq!(on.records, key_records());
    assert_eq!(again.records, on.records);
    assert!(off.records.is_empty(), "unset, no record is counted");
    assert_eq!(off.blobs, on.blobs, "the records reach no blob");
}
