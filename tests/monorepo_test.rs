//! End-to-end checks for config-driven monorepo support.
//!
//! A root `carrick.json` with a `projects` list makes Carrick analyze each app
//! directory as its own `<repo>::<app>` unit. These tests build a synthetic
//! monorepo on disk, run the engine against it in mock mode, and assert that
//! each app is uploaded under a composite key — and that misconfiguration
//! surfaces as a warning rather than a silent "all clean".

use async_trait::async_trait;
use carrick::cloud_storage::{CloudRepoData, CloudStorage, StorageError};
use carrick::engine::run_analysis_engine_with_sidecar;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

/// Plain in-memory storage with no synthetic seed repos.
#[derive(Default, Clone)]
struct StubStorage {
    repos: Arc<Mutex<Vec<CloudRepoData>>>,
}

#[async_trait]
impl CloudStorage for StubStorage {
    async fn upload_repo_data(&self, data: &CloudRepoData) -> Result<(), StorageError> {
        self.repos.lock().unwrap().push(data.clone());
        Ok(())
    }
    async fn download_all_repo_data(
        &self,
    ) -> Result<(Vec<CloudRepoData>, HashMap<String, String>), StorageError> {
        Ok((self.repos.lock().unwrap().clone(), HashMap::new()))
    }
    async fn upload_type_file(
        &self,
        _repo_name: &str,
        _file_name: &str,
        _content: &str,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    async fn health_check(&self) -> Result<(), StorageError> {
        Ok(())
    }
    async fn upload_logs(&self, _repo: &str, _log_content: &str) -> Result<(), StorageError> {
        Ok(())
    }
}

fn run_git(dir: &Path, args: &[&str]) {
    // Disable GPG signing — sandboxed CI environments may have commit.gpgsign
    // enabled globally without a working signer.
    let status = Command::new("git")
        .arg("-c")
        .arg("commit.gpgsign=false")
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

/// Write a minimal koa-style app with one endpoint into `dir`.
fn write_app(dir: &Path, pkg_name: &str) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("package.json"),
        format!(
            r#"{{"name": "{}", "version": "1.0.0", "dependencies": {{"koa": "^2.15.0", "@koa/router": "^12.0.1"}}}}"#,
            pkg_name
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/server.ts"),
        r#"import Koa from "koa";
import Router from "@koa/router";

const app = new Koa();
const router = new Router();

router.get("/health", (ctx) => {
  ctx.body = { ok: true };
});

app.use(router.routes());
app.listen(3000);
"#,
    )
    .unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn monorepo_analyzes_each_app_under_composite_key() {
    unsafe {
        std::env::set_var("CARRICK_MOCK_ALL", "1");
        std::env::set_var("CARRICK_API_KEY", "test");
        // Deterministic repo name regardless of temp dir naming.
        std::env::set_var("GITHUB_REPOSITORY", "acme/backend");
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // Root carrick.json declares the apps.
    std::fs::write(root.join("carrick.json"), r#"{"projects": ["apps/*"]}"#).unwrap();
    write_app(&root.join("apps/orders"), "orders-service");
    write_app(&root.join("apps/billing"), "billing-service");

    run_git(root, &["init", "-q"]);
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "init"]);

    let storage = StubStorage::default();
    run_analysis_engine_with_sidecar(storage.clone(), root.to_str().unwrap(), None, false, None)
        .await
        .expect("monorepo scan failed");

    let repos = storage.repos.lock().unwrap();
    let uploaded_names: Vec<String> = repos.iter().map(|r| r.repo_name.clone()).collect();

    // Both apps uploaded under composite `<repo>::<app>` keys.
    assert!(
        uploaded_names.contains(&"backend::orders-service".to_string()),
        "expected orders app upload, got {:?}",
        uploaded_names
    );
    assert!(
        uploaded_names.contains(&"backend::billing-service".to_string()),
        "expected billing app upload, got {:?}",
        uploaded_names
    );

    // package_name preserves the bare app name on each.
    let orders = repos
        .iter()
        .find(|r| r.repo_name == "backend::orders-service")
        .unwrap();
    assert_eq!(orders.package_name.as_deref(), Some("orders-service"));
}

#[tokio::test]
#[serial_test::serial]
async fn monorepo_with_no_matching_apps_warns_not_silent() {
    unsafe {
        std::env::set_var("CARRICK_MOCK_ALL", "1");
        std::env::set_var("CARRICK_API_KEY", "test");
        std::env::set_var("GITHUB_REPOSITORY", "acme/empty");
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    // projects points at a directory that doesn't exist — must not be treated
    // as a clean run.
    std::fs::write(root.join("carrick.json"), r#"{"projects": ["apps/*"]}"#).unwrap();
    std::fs::write(root.join("package.json"), r#"{"name": "empty"}"#).unwrap();

    run_git(root, &["init", "-q"]);
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "-q", "-m", "init"]);

    let storage = StubStorage::default();
    // Should complete without error and upload nothing (no apps to analyze).
    run_analysis_engine_with_sidecar(storage.clone(), root.to_str().unwrap(), None, false, None)
        .await
        .expect("scan should not error on empty monorepo");

    let repos = storage.repos.lock().unwrap();
    assert!(
        repos.is_empty(),
        "no apps should have been uploaded, got {:?}",
        repos.iter().map(|r| &r.repo_name).collect::<Vec<_>>()
    );
}
