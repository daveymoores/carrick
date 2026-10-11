//! carrick#2207: `--repo <owner/name>` names the repository a scan compares
//! its stored blobs against, and `--no-upload` writes none.
//!
//! A scan run outside GitHub Actions has no `GITHUB_REPOSITORY`, so it used to
//! name itself after its folder. A checkout cloned into a folder not named like
//! the repository then found no previous generation (every file went to the
//! model again) and kept its own stored copy among the siblings it is joined
//! against.
//!
//! Drives the real binary on the offline harness: `CARRICK_LOCAL_STORAGE_DIR`
//! as the store, the fixture's own cassette as the model, no cloud, no spend.
//! One generation is stored from a folder named `api`; the same commit is then
//! scanned from a folder named `checkout`.

use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE: &str = "env-var-whole-url";

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(FIXTURE)
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

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

/// What one scan printed and how it exited.
struct Scan {
    output: String,
    success: bool,
}

impl Scan {
    /// The files this scan sent to the analyzer, from its boundary line.
    fn files_sent(&self) -> usize {
        let line = Regex::new(r"(\d+) file\(s\) sent to the analyzer").unwrap();
        line.captures(&self.output)
            .unwrap_or_else(|| panic!("no boundary line:\n{}", self.output))[1]
            .parse()
            .unwrap()
    }
}

fn scan(repo: &Path, storage: &Path, flags: &[&str]) -> Scan {
    scan_with_env(repo, storage, flags, &[])
}

fn scan_with_env(repo: &Path, storage: &Path, flags: &[&str], env: &[(&str, &Path)]) -> Scan {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_carrick"));
    cmd.arg(repo)
        .args(flags)
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", fixture().join("__llm__").display()),
        )
        .env("CARRICK_SKIP_INTENTS", "1")
        .env("CARRICK_LOCAL_STORAGE_DIR", storage);
    for (name, value) in env {
        cmd.env(name, value);
    }
    // CI sets these to the repo being built, and the first of them would name
    // every scan here `carrick`.
    for var in [
        "GITHUB_REPOSITORY",
        "GITHUB_EVENT_NAME",
        "GITHUB_REF",
        "GITHUB_RUN_ID",
        "GITHUB_EVENT_PATH",
        "GITHUB_ACTIONS",
        "CI",
        "CARRICK_OUTPUT_JSON",
        "CARRICK_LOCAL_STORAGE_ISOLATE",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    ] {
        cmd.env_remove(var);
    }
    let result = cmd.output().expect("failed to spawn carrick");
    Scan {
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        ),
        success: result.status.success(),
    }
}

/// One generation stored from a folder named `api`, and a copy of the same
/// commit in a folder named `checkout`. Returns (`checkout`, the store).
fn stored_then_moved(tmp: &Path) -> (PathBuf, PathBuf) {
    let api = tmp.join("api");
    copy_dir(&fixture().join("src"), &api.join("src"));
    run_git(&api, &["init", "-q"]);
    run_git(&api, &["add", "-A"]);
    run_git(&api, &["commit", "-q", "-m", "init"]);

    let storage = tmp.join("store");
    std::fs::create_dir_all(&storage).unwrap();
    let first = scan(&api, &storage, &[]);
    assert!(first.success, "the first scan failed:\n{}", first.output);
    assert!(
        first.files_sent() > 0,
        "the first scan must ask the model, or a replay proves nothing:\n{}",
        first.output
    );

    let checkout = tmp.join("checkout");
    copy_dir(&api, &checkout);
    // A copy rewrites every mtime: refresh the index so the tree reads clean.
    run_git(&checkout, &["update-index", "-q", "--refresh"]);
    (checkout, storage)
}

/// With `--repo`, the moved checkout is the stored repository: every answer
/// replays, and its stored copy is its own rather than a sibling's.
#[test]
fn a_declared_repository_reuses_its_stored_generation_from_any_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let (checkout, storage) = stored_then_moved(tmp.path());

    let second = scan(&checkout, &storage, &["--repo", "acme/api"]);
    assert!(second.success, "{}", second.output);
    assert_eq!(second.files_sent(), 0, "{}", second.output);
    // The stored copy left the sibling side: the report names the one repo
    // rather than counting two.
    assert!(second.output.contains("Carrick · api"), "{}", second.output);
    assert!(!second.output.contains("2 repos"), "{}", second.output);
}

/// Without it the folder names the repository, as it always has: the stored
/// generation is a stranger's, so the files go to the model again and the
/// stored copy is joined against as a sibling.
#[test]
fn without_the_flag_the_folder_names_the_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let (checkout, storage) = stored_then_moved(tmp.path());

    let second = scan(&checkout, &storage, &[]);
    assert!(second.success, "{}", second.output);
    assert!(second.files_sent() > 0, "{}", second.output);
    assert!(second.output.contains("2 repos"), "{}", second.output);
}

/// `--no-upload` writes nothing into a store that would otherwise be filled.
#[test]
fn no_upload_leaves_the_store_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let api = tmp.path().join("api");
    copy_dir(&fixture().join("src"), &api.join("src"));
    run_git(&api, &["init", "-q"]);
    run_git(&api, &["add", "-A"]);
    run_git(&api, &["commit", "-q", "-m", "init"]);
    let storage = tmp.path().join("store");
    std::fs::create_dir_all(&storage).unwrap();

    let run = scan(&api, &storage, &["--no-upload"]);
    assert!(run.success, "{}", run.output);
    let written: Vec<_> = std::fs::read_dir(&storage).unwrap().flatten().collect();
    assert!(written.is_empty(), "{written:?}\n{}", run.output);
}

/// The two usage errors exit 2 with their own words, before any scan starts.
#[test]
fn the_runner_flags_refuse_what_they_cannot_do() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .args(["--repo", "api"])
        .arg(tmp.path())
        .env("CARRICK_MOCK_ALL", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("carrick: --repo takes owner/name (for example acme/api), not 'api'."),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new(env!("CARGO_BIN_EXE_carrick"))
        .arg(tmp.path())
        .arg("--upload")
        .env("CARRICK_OUTPUT_JSON", "1")
        .env("CARRICK_MOCK_ALL", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "carrick: --upload cannot be combined with CARRICK_OUTPUT_JSON, which never \
             uploads. Unset one of them."
        ),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git failed to spawn");
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A repository with base commit B0, a branch head H1 and a merge commit M of
/// the two, checked out at M. Returns (repo, B0, H1).
fn merged_pr(tmp: &Path) -> (PathBuf, String, String) {
    let repo = tmp.join("api");
    copy_dir(&fixture().join("src"), &repo.join("src"));
    run_git(&repo, &["init", "-q", "-b", "main"]);
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "b0"]);
    let b0 = git_out(&repo, &["rev-parse", "HEAD"]);
    run_git(&repo, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.join("notes.txt"), "pr").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "h1"]);
    let h1 = git_out(&repo, &["rev-parse", "HEAD"]);
    run_git(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("main-side.txt"), "main").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "main moves"]);
    run_git(&repo, &["merge", "-q", "--no-ff", "-m", "m", "feature"]);
    (repo, b0, h1)
}

/// carrick#2229: a pull request scan outside Actions takes its number and head
/// from the flags, posts its result without a run id, and writes no blob.
#[test]
fn a_pr_scan_takes_its_context_from_the_flags_and_writes_no_index() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _b0, h1) = merged_pr(tmp.path());
    let storage = tmp.path().join("store");
    std::fs::create_dir_all(&storage).unwrap();
    let out = tmp.path().join("pr-result.json");
    let base = "0".repeat(40);

    let run = scan_with_env(
        &repo,
        &storage,
        &["--pr", "7", "--pr-head", &h1, "--pr-base", &base],
        &[("CARRICK_PR_RESULT_OUT", &out)],
    );
    assert!(run.success, "{}", run.output);

    let posted: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("a PR run posts its result"))
            .unwrap();
    assert_eq!(posted["pr_number"], 7);
    assert_eq!(posted["head_sha"], h1.as_str());
    assert!(posted.get("run_id").is_none(), "{posted:#}");
    let written: Vec<_> = std::fs::read_dir(&storage).unwrap().flatten().collect();
    assert!(written.is_empty(), "{written:?}\n{}", run.output);
}

/// The flags are all-or-none, validated, and not combined with `--upload`;
/// a checkout that is not the PR's tree is refused. Each exits 2, with its
/// own words, before any scan starts.
#[test]
fn the_pr_flags_refuse_what_they_cannot_do() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, b0, h1) = merged_pr(tmp.path());
    let storage = tmp.path().join("store");
    std::fs::create_dir_all(&storage).unwrap();
    let elsewhere = "2".repeat(40);

    let refused = |flags: &[&str], expected: &str| {
        let run = scan(&repo, &storage, flags);
        assert!(!run.success, "{flags:?}\n{}", run.output);
        assert!(
            run.output.contains(expected),
            "{flags:?}: wanted {expected:?}\n{}",
            run.output
        );
    };
    refused(
        &["--pr", "7"],
        "carrick: --pr, --pr-head and --pr-base go together; pass all three.",
    );
    refused(
        &["--pr", "0", "--pr-head", &h1, "--pr-base", &b0],
        "carrick: --pr takes a pull request number, not '0'.",
    );
    refused(
        &["--pr", "7", "--pr-head", "abc", "--pr-base", &b0],
        "carrick: --pr-head and --pr-base take a full commit id (40 hex characters), not 'abc'.",
    );
    refused(
        &["--pr", "7", "--pr-head", &h1, "--pr-base", &b0, "--upload"],
        "carrick: --upload cannot be combined with --pr: a pull request scan never writes the index.",
    );
    refused(
        &["--pr", "7", "--pr-head", &elsewhere, "--pr-base", &b0],
        &format!(
            "carrick: this checkout is neither the pull request's head ({elsewhere}) nor a merge \
             of it, so its results would describe another tree."
        ),
    );
    let written: Vec<_> = std::fs::read_dir(&storage).unwrap().flatten().collect();
    assert!(written.is_empty(), "{written:?}");
}
