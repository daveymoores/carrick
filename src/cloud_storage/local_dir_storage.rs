//! On-disk `CloudStorage` for the offline cross-repo eval harness.
//!
//! `LocalDirStorage` is the storage backend that lets the eval harness run the
//! real scanner binary in two phases without ever touching the carrick cloud:
//!
//! - **Phase A (isolation):** each corpus repo is scanned in its own subprocess
//!   with `CARRICK_LOCAL_STORAGE_ISOLATE=1`, so `download_all_repo_data` returns
//!   *empty* — no real-cloud sibling data (and no other corpus repo) leaks into
//!   the run. `upload_repo_data` serialises that repo's [`CloudRepoData`] to
//!   `<dir>/<repo>.json`.
//! - **Phase B (join):** the binary runs once more without the isolate flag, so
//!   `download_all_repo_data` reads back *all* the cached repos. The engine's
//!   `build_cross_repo_analyzer` then joins them exactly as the cloud path would.
//!
//! The backend is chosen at binary startup purely by the presence of the
//! `CARRICK_LOCAL_STORAGE_DIR` env var (see `main.rs`). The engine never learns
//! it is in eval mode — same contract as `MockStorage`.

use crate::cloud_storage::{CloudRepoData, CloudStorage, StorageError};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;

/// Env var holding the cache directory. Its presence at startup also selects
/// this backend over `MockStorage`/`AwsStorage`.
pub const CACHE_DIR_ENV: &str = "CARRICK_LOCAL_STORAGE_DIR";
/// When set to `1`, `download_all_repo_data` returns empty (Phase A isolation).
pub const ISOLATE_ENV: &str = "CARRICK_LOCAL_STORAGE_ISOLATE";

pub struct LocalDirStorage {
    cache_dir: PathBuf,
    isolate: bool,
}

impl LocalDirStorage {
    /// Construct from the `CARRICK_LOCAL_STORAGE_DIR` / `CARRICK_LOCAL_STORAGE_ISOLATE`
    /// env vars. Creates the cache dir if it does not exist.
    pub fn from_env() -> Result<Self, StorageError> {
        let cache_dir = std::env::var(CACHE_DIR_ENV)
            .map_err(|_| StorageError::ConnectionError(format!("{CACHE_DIR_ENV} is not set")))?;
        let isolate = std::env::var(ISOLATE_ENV).as_deref() == Ok("1");
        Self::new(PathBuf::from(cache_dir), isolate)
    }

    pub fn new(cache_dir: PathBuf, isolate: bool) -> Result<Self, StorageError> {
        std::fs::create_dir_all(&cache_dir).map_err(|e| {
            StorageError::ConnectionError(format!(
                "Failed to create local storage dir {}: {e}",
                cache_dir.display()
            ))
        })?;
        Ok(Self { cache_dir, isolate })
    }

    /// Sanitize a repo name into a single-segment file stem so a repo id that
    /// contains a path separator can't escape the cache dir.
    fn cache_path(&self, repo_name: &str) -> PathBuf {
        let safe: String = repo_name
            .chars()
            .map(|c| if c == '/' || c == '\\' { '_' } else { c })
            .collect();
        self.cache_dir.join(format!("{safe}.json"))
    }
}

#[async_trait]
impl CloudStorage for LocalDirStorage {
    async fn upload_repo_data(&self, data: &CloudRepoData) -> Result<(), StorageError> {
        let path = self.cache_path(&data.repo_name);
        debug!(
            "LOCAL: Uploading repo data for {} (service: {:?}) -> {}",
            data.repo_name,
            data.service_name,
            path.display()
        );
        let json = serde_json::to_string_pretty(data)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        std::fs::write(&path, json).map_err(|e| {
            StorageError::ConnectionError(format!("Failed to write {}: {e}", path.display()))
        })?;
        Ok(())
    }

    // Cache entries are keyed per file (one JSON per repo id), so multiple
    // services upload without clobbering — same property as MockStorage.
    fn supports_multi_service(&self) -> bool {
        true
    }

    async fn download_all_repo_data(
        &self,
    ) -> Result<(Vec<CloudRepoData>, HashMap<String, String>), StorageError> {
        // Phase A: upload-only. Returning empty is the load-bearing isolation —
        // without it the real cloud (or a sibling corpus repo) would inject data
        // into the per-repo scan and break Tier-A fidelity.
        if self.isolate {
            debug!("LOCAL: isolate mode — returning empty cross-repo set");
            return Ok((Vec::new(), HashMap::new()));
        }

        let mut repos = Vec::new();
        let entries = std::fs::read_dir(&self.cache_dir).map_err(|e| {
            StorageError::ConnectionError(format!(
                "Failed to read local storage dir {}: {e}",
                self.cache_dir.display()
            ))
        })?;
        // Collect + sort paths so the joined order is deterministic across runs.
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
            .collect();
        paths.sort();
        for path in paths {
            let content = std::fs::read_to_string(&path).map_err(|e| {
                StorageError::ConnectionError(format!("Failed to read {}: {e}", path.display()))
            })?;
            let data: CloudRepoData = serde_json::from_str(&content).map_err(|e| {
                StorageError::SerializationError(format!("Failed to parse {}: {e}", path.display()))
            })?;
            repos.push(data);
        }

        // S3 URL map is unused offline; supply a stable local marker per repo so
        // any consumer expecting a key per repo still finds one.
        let urls = repos
            .iter()
            .map(|r| (r.repo_name.clone(), format!("file://{}", r.repo_name)))
            .collect();

        debug!("LOCAL: Downloaded {} cached repos", repos.len());
        Ok((repos, urls))
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        debug!("LOCAL: Health check passed");
        Ok(())
    }

    async fn upload_logs(&self, repo: &str, _log_content: &str) -> Result<(), StorageError> {
        debug!("LOCAL: Skipping log upload for {}", repo);
        Ok(())
    }

    async fn upload_type_file(
        &self,
        repo_name: &str,
        file_name: &str,
        _content: &str,
    ) -> Result<(), StorageError> {
        debug!(
            "LOCAL: Skipping type-file upload for {} / {}",
            repo_name, file_name
        );
        Ok(())
    }

    async fn post_pr_comment(
        &self,
        repo: &str,
        pr_number: u64,
        _run_id: &str,
        _body: &str,
    ) -> Result<(), StorageError> {
        debug!(
            "LOCAL: Skipping PR comment for {} (PR #{})",
            repo, pr_number
        );
        Ok(())
    }
}
