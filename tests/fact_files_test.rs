//! carrick#2613 (ticket T1): the fact layer's files are worked out before the
//! file reader dispatches, from the library store's first answer, and
//! recorded in the run log. Record-only: which file is read does not move.
//!
//! The fixture is the library-store one, scanned as the Action scans it
//! (`CARRICK_POSITION_ROWS=1`, `CARRICK_SITE_QUESTIONS=1`, the real sidecar,
//! the store's answer from the cassette), with two files added here:
//! - `src/notify.ts`, whose only network call is `relay(url)` on a labelled
//!   default export. It is not awaited, takes no literal argument and names
//!   nothing a signal looks for, so no hand-written signal raises it and
//!   today the reader never sees the file;
//! - `src/slow.ts`, whose only call goes through a package the store has not
//!   answered yet.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap();
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

/// A package installed in the fixture's `node_modules`, locked to the public
/// registry, with one default export declared.
fn add_package(repo: &Path, name: &str, version: &str) {
    let dir = repo.join("node_modules").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("package.json"),
        format!(r#"{{ "name": "{name}", "version": "{version}", "types": "index.d.ts" }}"#),
    )
    .unwrap();
    fs::write(
        dir.join("index.d.ts"),
        "declare function send(target: string): void;\nexport default send;\n",
    )
    .unwrap();
    let manifest = repo.join("package.json");
    let mut package: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    package["dependencies"][name] = format!("^{version}").into();
    fs::write(&manifest, package.to_string()).unwrap();
    let lock = repo.join("package-lock.json");
    let mut locked: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&lock).unwrap()).unwrap();
    locked["packages"][format!("node_modules/{name}")] = serde_json::json!({
        "version": version,
        "resolved": format!("https://registry.npmjs.org/{name}/-/x-{version}.tgz"),
        "integrity": "sha512-fixture",
    });
    fs::write(&lock, locked.to_string()).unwrap();
}

/// The fixture, committed, with `relay` labelled by the store and `slow` left
/// pending.
fn repo(tmp: &Path) -> PathBuf {
    let repo = tmp.join("relay-service");
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library-store"),
        &repo,
    );
    fs::remove_file(repo.join("README.md")).ok();
    add_package(&repo, "@fixture/relay", "1.0.0");
    add_package(&repo, "@fixture/slow", "1.0.0");
    fs::write(
        repo.join("src/notify.ts"),
        "import relay from \"@fixture/relay\";\n\nexport function ping(url: string): void {\n  relay(url);\n}\n",
    )
    .unwrap();
    fs::write(
        repo.join("src/slow.ts"),
        "import slow from \"@fixture/slow\";\n\nexport function later(target: string): void {\n  slow(target);\n}\n",
    )
    .unwrap();
    let cassette = repo.join("__llm__/library-claims/default.json");
    let mut answer: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cassette).unwrap()).unwrap();
    let claims = answer["library_claims"].as_array_mut().unwrap();
    claims.push(serde_json::json!({
        "package": "@fixture/relay", "version": "1.0.0", "status": "answered",
        "surface_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "exports": [{
            "specifier": "@fixture/relay", "export": "default", "role": "http_client",
            "side": null, "classifier": "fixture-classifier", "makes": [], "scopes": [],
            "ops": [], "reserved": [], "calls": [], "patterns": []
        }]
    }));
    claims.push(serde_json::json!({
        "package": "@fixture/slow", "version": "1.0.0", "status": "pending",
        "reason": "lister_unavailable"
    }));
    fs::write(&cassette, answer.to_string()).unwrap();
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["add", "-A", "-f"]);
    run_git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

/// The scan's run log, with the fact layer on or off.
fn scan_log(site_questions: bool) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path());
    let storage = tmp.path().join("storage");
    let home = tmp.path().join("home");
    fs::create_dir_all(&storage).unwrap();
    fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_carrick"));
    cmd.arg(&repo)
        .env("HOME", &home)
        .env("CARRICK_LOCAL_STORAGE_DIR", &storage)
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_CACHE_DIR", tmp.path().join("cache"))
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", repo.join("__llm__").display()),
        )
        .env("CARRICK_SKIP_INTENTS", "1")
        .env(
            "CARRICK_SIDECAR_DIR",
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/sidecar"),
        )
        .env("CARRICK_POSITION_ROWS", "1");
    if site_questions {
        cmd.env("CARRICK_SITE_QUESTIONS", "1");
    } else {
        cmd.env_remove("CARRICK_SITE_QUESTIONS");
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
        "CARRICK_ALLOW_MISSING_TYPES",
    ] {
        cmd.env_remove(var);
    }
    let output = cmd.output().expect("spawn carrick");
    assert!(
        output.status.success(),
        "scan exited non-zero:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read_dir(home.join(".carrick/logs/runs"))
        .expect("the scan keeps a run log")
        .filter_map(|entry| fs::read_to_string(entry.ok()?.path()).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

fn fact_lines(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|line| line.contains("fact files:"))
        .collect()
}

#[test]
fn files_only_the_fact_layer_finds_are_logged_as_fact_only() {
    let log = scan_log(true);
    let lines = fact_lines(&log);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("fact-only src/notify.ts (site true")),
        "the file whose only call is a labelled relay is fact-only: {lines:#?}"
    );
    assert!(
        lines.iter().any(|line| {
            line.contains("fact-only src/slow.ts (site false, lead false, unanswered-package true)")
        }),
        "a call into a package the store has not answered selects its file: {lines:#?}"
    );
    assert!(
        lines.iter().any(|line| {
            line.contains("fact files: ")
                && line.contains("read today")
                && line.contains("fact-only")
        }),
        "the per-service summary line: {lines:#?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("packages unanswered at dispatch 1")),
        "{lines:#?}"
    );
    for signal in ["C1", "C2", "C3", "C4"] {
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("{signal} candidates: at a site"))),
            "{signal} is counted: {lines:#?}"
        );
    }
    // Record-only: the reader was sent neither file.
    for file in ["src/notify.ts", "src/slow.ts"] {
        assert!(
            !log.lines().any(|line| {
                line.contains(file)
                    && (line.contains("Analyzing:") || line.contains("Force-analyzing"))
            }),
            "the reader's file set did not move for {file}"
        );
    }
    assert!(
        log.lines().any(
            |line| line.contains("Skipped (no API patterns)") && line.contains("src/notify.ts")
        ),
        "today's gate still skips the file"
    );
}

#[test]
fn with_the_site_questions_off_nothing_is_recorded_and_the_scan_runs() {
    let log = scan_log(false);
    assert!(
        fact_lines(&log).is_empty(),
        "no fact-layer record without the site questions: {:#?}",
        fact_lines(&log)
    );
}
