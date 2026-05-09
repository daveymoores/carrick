//! End-to-end check for issue #110: the incremental scan path must populate
//! `FunctionDefinition.intent` and strip `body_source`, just like the full
//! analysis path does. Before the fix, the incremental path skipped intent
//! generation entirely, leaving DDB rows with `body_source` populated and
//! `intent` absent.

use async_trait::async_trait;
use carrick::cloud_storage::{CloudRepoData, CloudStorage, MockStorage, StorageError};
use carrick::engine::run_analysis_engine_with_sidecar;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

#[derive(Clone)]
struct SharedMock(Arc<MockStorage>);

#[async_trait]
impl CloudStorage for SharedMock {
    async fn upload_repo_data(&self, data: &CloudRepoData) -> Result<(), StorageError> {
        self.0.upload_repo_data(data).await
    }
    async fn download_all_repo_data(
        &self,
    ) -> Result<(Vec<CloudRepoData>, HashMap<String, String>), StorageError> {
        self.0.download_all_repo_data().await
    }
    async fn upload_type_file(
        &self,
        repo_name: &str,
        file_name: &str,
        content: &str,
    ) -> Result<(), StorageError> {
        self.0.upload_type_file(repo_name, file_name, content).await
    }
    async fn health_check(&self) -> Result<(), StorageError> {
        self.0.health_check().await
    }
    async fn upload_logs(&self, repo: &str, log_content: &str) -> Result<(), StorageError> {
        self.0.upload_logs(repo, log_content).await
    }
}

fn run_git(dir: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .expect("git failed to spawn");
    assert!(status.success(), "git {:?} failed", args);
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) {
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

#[tokio::test]
async fn incremental_path_populates_intent() {
    // Mock all lambda + storage interactions.
    // SAFETY: tests in this binary share env, but no other test in this file
    // depends on these vars.
    unsafe {
        std::env::set_var("CARRICK_MOCK_ALL", "1");
        std::env::set_var("CARRICK_API_KEY", "test");
    }

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/koa-api");
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_path = tmp.path().join("koa");
    copy_dir(&fixture, &repo_path);

    run_git(&repo_path, &["init", "-q"]);
    run_git(&repo_path, &["add", "-A"]);
    run_git(&repo_path, &["commit", "-q", "-m", "init"]);

    let storage = SharedMock(Arc::new(MockStorage::new()));

    // Scan #1 — full path. Populates cache fields so scan #2 takes the
    // incremental branch.
    run_analysis_engine_with_sidecar(
        storage.clone(),
        repo_path.to_str().unwrap(),
        None,
        false,
        None,
    )
    .await
    .expect("scan #1 failed");

    // Make a trivial change so HEAD differs from the previous commit_hash —
    // git diff in the incremental path needs a non-empty diff target.
    std::fs::write(repo_path.join("noop.ts"), "// touch\n").unwrap();
    run_git(&repo_path, &["add", "-A"]);
    run_git(&repo_path, &["commit", "-q", "-m", "noop"]);

    // Scan #2 — should take the incremental branch.
    run_analysis_engine_with_sidecar(
        storage.clone(),
        repo_path.to_str().unwrap(),
        None,
        false,
        None,
    )
    .await
    .expect("scan #2 failed");

    // Inspect the most-recently uploaded payload for this repo.
    let (uploaded, _) = storage
        .0
        .download_all_repo_data()
        .await
        .expect("download failed");

    // download_all_repo_data adds synthetic seed repos when total <= 1, so
    // filter for our actual repo by package.json signature.
    let our_repo = uploaded
        .iter()
        .rfind(|r| {
            r.package_json
                .as_ref()
                .map(|j| j.contains("koa"))
                .unwrap_or(false)
        })
        .expect("no koa repo upload found");

    // After the fix, the latest (incremental) upload must have intents
    // populated and body_source stripped, same as the full-path upload would.
    let with_intent = our_repo
        .function_definitions
        .values()
        .filter(|d| d.intent.is_some())
        .count();
    let with_body = our_repo
        .function_definitions
        .values()
        .filter(|d| d.body_source.is_some())
        .count();

    assert!(
        with_intent > 0,
        "incremental upload should populate intent on at least one function (got {} of {})",
        with_intent,
        our_repo.function_definitions.len()
    );
    assert_eq!(
        with_body, 0,
        "incremental upload should strip body_source on all functions (still set on {})",
        with_body
    );
}
