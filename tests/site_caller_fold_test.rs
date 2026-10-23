//! carrick#2614: a reader row at a call of an in-repo function folds onto
//! the request that function states.
//!
//! Drives the real scanner binary offline over `tests/fixtures/caller-fold/`
//! as the Action runs it, with the compiler's position rows and the site
//! questions on (`CARRICK_POSITION_ROWS=1`, `CARRICK_SITE_QUESTIONS=1`) and
//! the type sidecar live.
//!
//! `src/client.ts` declares `login` and `logout`, each a call of a local
//! `send(path, body)` that hands its arguments to `http.post` on a package
//! the library store labels an HTTP client. Each path is decided at the
//! `send` call that writes it (`/api/login` in `login`; `/api/logout` and
//! `/api/audit` in `logout`), so the fact layer places each request there.
//! `src/page.ts` awaits `login(...)` and `logout(...)`. The reader's cassette
//! states a request at all five lines.
//!
//! Answer key (a row "stands" whatever pass states it; at the callee lines
//! the scan's own rows keep the site today):
//!
//! | line | reader states | switch off | switch on |
//! |---|---|---|---|
//! | `src/client.ts:12` | `POST /api/login` | a row | a row |
//! | `src/client.ts:16`, `:17` | `POST /api/logout`, `POST /api/audit` | a row each | a row each |
//! | `src/page.ts:4` (calls `login`) | `POST /api/login` | the reader's row | folded: no row |
//! | `src/page.ts:9` (calls `logout`) | `DELETE /api/session` | the reader's row | the reader's row: the callee states other operations |

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .status()
        .expect("git failed to spawn");
    assert!(status.success(), "git {args:?} failed");
}

/// The fixture, copied and committed as a repo of its own.
fn fixture_repo(tmp: &Path) -> PathBuf {
    let repo = tmp.join("caller-fold");
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/caller-fold"),
        &repo,
    );
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["add", "-A", "-f"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

/// The blob one offline scan of `repo` stores, with the site questions on
/// or off.
fn scan(repo: &Path, site_questions: bool) -> serde_json::Value {
    let storage = tempfile::tempdir().expect("temp storage dir");
    let cache = tempfile::tempdir().expect("temp cache dir");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_carrick"));
    cmd.arg(repo)
        .env("CARRICK_LOCAL_STORAGE_DIR", storage.path())
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_CACHE_DIR", cache.path())
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", repo.join("__llm__").display()),
        )
        .env("CARRICK_SKIP_INTENTS", "1")
        .env("CARRICK_POSITION_ROWS", "1")
        .env_remove("CARRICK_ALLOW_MISSING_TYPES")
        .env_remove("CARRICK_SITE_PAYLOADS")
        .env_remove("CARRICK_SITE_QUESTIONS");
    if site_questions {
        cmd.env("CARRICK_SITE_QUESTIONS", "1");
    }
    for var in [
        "GITHUB_REPOSITORY",
        "GITHUB_REF",
        "GITHUB_EVENT_NAME",
        "GITHUB_SHA",
        "GITHUB_RUN_ID",
        "GITHUB_ACTIONS",
        "GITHUB_WORKSPACE",
        "CI",
        "CARRICK_NO_MODEL",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    ] {
        cmd.env_remove(var);
    }
    let output = cmd.output().expect("failed to spawn carrick");
    assert!(
        output.status.success(),
        "scan exited non-zero:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut blobs: Vec<serde_json::Value> = fs::read_dir(storage.path())
        .expect("storage dir")
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .map(|entry| {
            serde_json::from_slice(&fs::read(entry.path()).expect("read blob")).expect("parse blob")
        })
        .collect();
    assert_eq!(blobs.len(), 1, "one service, one blob");
    blobs.remove(0)
}

/// Every call row as `(file:line, METHOD path, resolution source)`, sorted.
fn calls(blob: &serde_json::Value) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = blob["calls"]
        .as_array()
        .expect("calls")
        .iter()
        .map(|row| {
            (
                row["file_path"].as_str().unwrap_or_default().to_string(),
                format!(
                    "{} {}",
                    row["key"]["method"].as_str().unwrap_or_default(),
                    row["key"]["path"].as_str().unwrap_or_default()
                ),
                row["resolution_source"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

fn at<'r>(rows: &'r [(String, String, String)], place: &str) -> Vec<&'r (String, String, String)> {
    rows.iter().filter(|row| row.0 == place).collect()
}

#[test]
fn a_reader_row_at_a_call_of_a_function_that_states_the_request_folds_onto_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = fixture_repo(tmp.path());

    // Off: every line holds a row, the caller's copy of `/api/login` among
    // them. This is the switch-off output that must not move, and the proof
    // the reader states the second copy this test folds.
    let off = calls(&scan(&repo, false));
    let places = [
        "src/client.ts:12",
        "src/client.ts:16",
        "src/client.ts:17",
        "src/page.ts:4",
        "src/page.ts:9",
    ];
    for place in places {
        assert_eq!(at(&off, place).len(), 1, "{place} off: {off:#?}");
    }
    assert_eq!(off.len(), places.len(), "{off:#?}");
    let caller = at(&off, "src/page.ts:4");
    assert_eq!(
        (caller[0].1.as_str(), caller[0].2.as_str()),
        ("POST /api/login", "model"),
        "{off:#?}"
    );

    let on = calls(&scan(&repo, true));
    // The request stands once, at the call that decides its path.
    let login = at(&on, "src/client.ts:12");
    assert_eq!(login.len(), 1, "{on:#?}");
    assert_eq!(login[0].1, "POST /api/login", "{on:#?}");
    assert_ne!(login[0].2, "model", "{on:#?}");
    // The caller's copy of the same request is folded onto it.
    assert!(at(&on, "src/page.ts:4").is_empty(), "{on:#?}");
    // A caller whose reader row names another operation than any its callee
    // states keeps its row.
    let logout_caller = at(&on, "src/page.ts:9");
    assert_eq!(logout_caller.len(), 1, "{on:#?}");
    assert!(logout_caller[0].1.ends_with("/api/session"), "{on:#?}");
    assert_eq!(at(&on, "src/client.ts:16").len(), 1, "{on:#?}");
    assert_eq!(at(&on, "src/client.ts:17").len(), 1, "{on:#?}");
    assert_eq!(on.len(), places.len() - 1, "{on:#?}");
}
