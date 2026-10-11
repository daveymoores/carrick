//! Test-harness helpers shared by the integration tests.
//!
//! A scan reads its services from the project config `CARRICK_CONFIG` names,
//! never from a `carrick.json` in the tree (carrick#2272). A fixture's
//! committed `carrick.json` is the configuration its answer key was written
//! against, so a test that scans one hands that body to the scan the way the
//! Action and the runner do: as this repository's entry in a project config
//! document.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The `owner/name` a fixture directory is scanned as. Its basename is the
/// directory name, so the repository name a scan keys its blobs by is the
/// one it had before the name was declared.
pub fn fixture_repo(dir: &Path) -> String {
    let name = dir
        .file_name()
        .expect("a fixture directory has a name")
        .to_string_lossy()
        .into_owned();
    format!("fixtures/{name}")
}

/// A `carrick.project-config/1` document holding one entry per
/// `(owner/name, carrick.json body)`, written to a file that outlives the
/// test (it sits in the system temp directory).
pub fn project_config(entries: &[(String, serde_json::Value)]) -> PathBuf {
    let repos: Vec<serde_json::Value> = entries
        .iter()
        .enumerate()
        .map(|(index, (repo, config))| {
            serde_json::json!({
                "repo": repo,
                "github_repo_id": (index + 1).to_string(),
                "held": true,
                "behind": false,
                "config": config,
            })
        })
        .collect();
    let document = serde_json::json!({
        "schema": "carrick.project-config/1",
        "projects": [{ "id": "fixture-project", "version": 1, "repos": repos }],
    });
    let file = tempfile::Builder::new()
        .prefix("carrick-project-config-")
        .suffix(".json")
        .tempfile()
        .expect("create a project config file");
    std::fs::write(file.path(), document.to_string()).expect("write the project config");
    file.into_temp_path()
        .keep()
        .expect("keep the project config file")
}

/// The `carrick.json` body committed in `dir`, if there is one.
pub fn tree_config(dir: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(dir.join("carrick.json")).ok()?;
    Some(serde_json::from_str(&text).expect("a fixture's carrick.json parses"))
}

/// Scan `dir` as [`fixture_repo`], with its committed `carrick.json` (if any)
/// as that repository's project config entry. A directory without one is
/// scanned as before: nothing is declared and the services are derived.
pub fn with_tree_config<'a>(cmd: &'a mut Command, dir: &Path) -> &'a mut Command {
    let Some(config) = tree_config(dir) else {
        return cmd;
    };
    let repo = fixture_repo(dir);
    let path = project_config(&[(repo.clone(), config)]);
    cmd.arg("--repo").arg(repo).env("CARRICK_CONFIG", path)
}

/// Unsets `CARRICK_CONFIG` when dropped.
pub struct ConfigEnv;

impl Drop for ConfigEnv {
    fn drop(&mut self) {
        // SAFETY: only tests whose scans run one at a time hold this.
        unsafe { std::env::remove_var("CARRICK_CONFIG") };
    }
}

/// The project config document for a git checkout `repo` whose tree holds a
/// `carrick.json`: `repo` gets the origin remote of [`fixture_repo`], which is
/// the name it is matched by when nothing passes `--repo` (an in-process scan,
/// `carrick derive`, the Action's installer).
pub fn tree_config_for_checkout(repo: &Path) -> Option<PathBuf> {
    let config = tree_config(repo)?;
    let name = fixture_repo(repo);
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "add", "origin"])
        .arg(format!("https://github.com/{name}.git"))
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .status()
        .expect("run git");
    assert!(status.success(), "git remote add in {}", repo.display());
    Some(project_config(&[(name, config)]))
}

/// [`tree_config_for_checkout`] for a scan that runs in this process:
/// `CARRICK_CONFIG` is set until the guard drops.
///
/// Only for a test binary whose scans run one at a time: the variable is the
/// process's.
pub fn in_process_tree_config(repo: &Path) -> Option<ConfigEnv> {
    let path = tree_config_for_checkout(repo)?;
    // SAFETY: the caller's scans run one at a time (see above).
    unsafe { std::env::set_var("CARRICK_CONFIG", path) };
    Some(ConfigEnv)
}
