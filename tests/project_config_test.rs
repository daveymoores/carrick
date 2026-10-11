//! carrick#2272: a scan reads its services from the project config
//! `CARRICK_CONFIG` names, and never from a `carrick.json` in the tree.
//!
//! The repository is written by this test: `acme/mono` (its origin remote),
//! an npm workspace of `apps/web` and `services/api`, with a committed root
//! `carrick.json` that declares one root service. That file is never read.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

/// `acme/mono`, committed.
fn mono() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "package.json",
        r#"{"name":"mono","private":true,"workspaces":["apps/*","services/*"]}"#,
    );
    write(root, "apps/web/package.json", r#"{"name":"web"}"#);
    write(
        root,
        "apps/web/index.ts",
        "export function renderHome() { return 'home'; }\n",
    );
    write(root, "services/api/package.json", r#"{"name":"api"}"#);
    write(
        root,
        "services/api/index.ts",
        "export function listOrders() { return fetch(`${process.env.ORDERS_URL}/orders`); }\n",
    );
    write(
        root,
        "carrick.json",
        r#"{"serviceName":"declared-root","internalEnvVars":["TREE_ONLY_URL"]}"#,
    );
    git(root, &["init", "-q", "."]);
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/mono.git",
        ],
    );
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-qm",
            "init",
        ],
    );
    dir
}

/// A project config document of `(owner/name, config)` entries, `config`
/// null where the cloud holds nothing.
fn document(entries: &[(&str, serde_json::Value)]) -> PathBuf {
    support::project_config(
        &entries
            .iter()
            .map(|(repo, config)| (repo.to_string(), config.clone()))
            .collect::<Vec<_>>(),
    )
}

fn carrick(args: &[&str], config: Option<&Path>) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_carrick"));
    cmd.args(args);
    for var in [
        "GITHUB_REPOSITORY",
        "GITHUB_REF",
        "GITHUB_EVENT_NAME",
        "GITHUB_SHA",
        "GITHUB_RUN_ID",
        "GITHUB_ACTIONS",
        "GITHUB_WORKSPACE",
        "CI",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
        "CARRICK_CONFIG",
    ] {
        cmd.env_remove(var);
    }
    if let Some(config) = config {
        cmd.env("CARRICK_CONFIG", config);
    }
    cmd
}

struct Scan {
    output: Output,
    /// Each blob's service name and its parsed `config_json`.
    blobs: BTreeMap<String, serde_json::Value>,
}

impl Scan {
    fn text(&self) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&self.output.stdout),
            String::from_utf8_lossy(&self.output.stderr)
        )
    }

    fn services(&self) -> Vec<&str> {
        self.blobs.keys().map(String::as_str).collect()
    }
}

/// An offline scan of `repo`, with `extra` arguments after the path.
fn scan(repo: &Path, extra: &[&str], config: Option<&Path>) -> Scan {
    let storage = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let cassettes = tempfile::tempdir().unwrap();
    let mut args = vec![repo.to_str().unwrap()];
    args.extend_from_slice(extra);
    let output = carrick(&args, config)
        .env("CARRICK_LOCAL_STORAGE_DIR", storage.path())
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_CACHE_DIR", cache.path())
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", cassettes.path().display()),
        )
        .env("CARRICK_SKIP_INTENTS", "1")
        .env("CARRICK_ALLOW_MISSING_TYPES", "1")
        .output()
        .expect("spawn carrick");
    let mut blobs = BTreeMap::new();
    if output.status.success() {
        for entry in std::fs::read_dir(storage.path()).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let blob: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let Some(service) = blob["service_name"].as_str() else {
                continue;
            };
            let config = blob["config_json"]
                .as_str()
                .map(|text| serde_json::from_str(text).unwrap())
                .unwrap_or(serde_json::Value::Null);
            blobs.insert(service.to_string(), config);
        }
    }
    Scan { output, blobs }
}

fn api_section() -> serde_json::Value {
    json!({"services":[
        {"serviceName":"api","directory":"services/api","internalEnvVars":["ORDERS_URL"]}
    ]})
}

/// Test 1. The break test: restore the tree read in `resolve` and this fails.
#[test]
fn without_a_project_config_the_tree_carrick_json_is_ignored() {
    let repo = mono();
    let scan = scan(repo.path(), &[], None);
    assert!(scan.output.status.success(), "{}", scan.text());
    assert_eq!(scan.services(), ["api", "web"], "{}", scan.text());
    assert!(!scan.text().contains("TREE_ONLY_URL"));
}

/// Test 2. The blob's `config_json` is the section's entry.
#[test]
fn a_section_names_the_services_and_their_declarations() {
    let repo = mono();
    let config = document(&[("acme/web-only", json!({})), ("acme/mono", api_section())]);
    let scan = scan(repo.path(), &[], Some(&config));
    assert!(scan.output.status.success(), "{}", scan.text());
    assert_eq!(scan.services(), ["api"], "{}", scan.text());
    let stored = &scan.blobs["api"];
    assert_eq!(stored["serviceName"], "api");
    assert_eq!(stored["directory"], "services/api");
    assert_eq!(stored["internalEnvVars"], json!(["ORDERS_URL"]));
}

/// Test 3. A member whose directory this tree lacks is left out, silently.
#[test]
fn a_section_member_this_tree_lacks_is_left_out() {
    let repo = mono();
    let config = document(&[(
        "acme/mono",
        json!({"services":[
            {"serviceName":"api","directory":"services/api"},
            {"serviceName":"gone","directory":"services/gone"}
        ]}),
    )]);
    let scan = scan(repo.path(), &[], Some(&config));
    assert!(scan.output.status.success(), "{}", scan.text());
    assert_eq!(scan.services(), ["api"]);
    assert!(!scan.text().contains("services/gone"), "{}", scan.text());
}

/// Test 4. No entry derives; two entries or another schema is an error.
#[test]
fn no_entry_derives_and_an_unreadable_document_is_an_error() {
    let repo = mono();
    let other = document(&[("acme/other", api_section())]);
    let scan_other = scan(repo.path(), &[], Some(&other));
    assert!(scan_other.output.status.success(), "{}", scan_other.text());
    assert_eq!(scan_other.services(), ["api", "web"]);

    let null = document(&[("acme/mono", serde_json::Value::Null)]);
    let scan_null = scan(repo.path(), &[], Some(&null));
    assert!(scan_null.output.status.success(), "{}", scan_null.text());
    assert_eq!(scan_null.services(), ["api", "web"]);

    let two = document(&[("acme/mono", api_section()), ("ACME/Mono", json!({}))]);
    let scan_two = scan(repo.path(), &[], Some(&two));
    assert!(!scan_two.output.status.success());
    assert!(
        scan_two.text().contains("2 entries for acme/mono"),
        "{}",
        scan_two.text()
    );

    let wrong = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        wrong.path(),
        json!({"schema":"carrick.project-config/0","projects":[]}).to_string(),
    )
    .unwrap();
    let scan_wrong = scan(repo.path(), &[], Some(wrong.path()));
    assert!(!scan_wrong.output.status.success());
    assert!(
        scan_wrong.text().contains("carrick.project-config/1"),
        "{}",
        scan_wrong.text()
    );
}

/// Test 5. `--repo` names the entry, case-insensitively, over the remote.
#[test]
fn the_declared_repo_picks_its_entry_over_the_remote() {
    let repo = mono();
    let config = document(&[
        ("acme/mono", json!({"serviceName":"from-remote"})),
        (
            "acme/other",
            json!({"services":[{"serviceName":"web-from-flag","directory":"apps/web"}]}),
        ),
    ]);
    let scan = scan(repo.path(), &["--repo", "acme/Other"], Some(&config));
    assert!(scan.output.status.success(), "{}", scan.text());
    assert_eq!(scan.services(), ["web-from-flag"]);
}

/// Test 9. `derive` reports what a scan would read.
#[test]
fn derive_reports_the_section_a_scan_would_read() {
    let repo = mono();
    let config = document(&[("acme/mono", api_section())]);
    let output = carrick(
        &[
            "derive",
            "--workspace",
            repo.path().to_str().unwrap(),
            "--json",
            "--candidates",
        ],
        Some(&config),
    )
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let derived: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let entry = &derived["repos"][0];
    assert_eq!(entry["reason"], "project config");
    let services = entry["services"].as_array().unwrap();
    assert_eq!(services.len(), 1, "{entry}");
    assert_eq!(services[0]["serviceName"], "api");
    assert_eq!(services[0]["internalEnvVars"], json!(["ORDERS_URL"]));
    assert_eq!(services[0]["candidates"]["env"][0]["name"], "ORDERS_URL");
}

/// The bytes `carrick derive --json` printed on main before carrick#2272 for
/// [`mono`] without its `carrick.json`, with the root path as `<ROOT>`. The
/// cloud planner hashes these, so they do not move.
const DERIVE_GOLDEN: &str = r#"{"missing":[],"parent_proposal":null,"repos":[{"config":{"services":[{"directory":"apps/web","externalDomains":[],"externalEnvVars":[],"internalDomains":[],"internalEnvVars":[],"serviceName":"web"},{"directory":"services/api","externalDomains":[],"externalEnvVars":[],"internalDomains":[],"internalEnvVars":[],"serviceName":"api"}]},"not_installed":[],"notes":["Workspace packages are proposed as services. Review service boundaries and shared source includes in carrick.json."],"path":"<ROOT>","reason":"npm workspaces","services":[{"bin":false,"deploy_config":[],"directory":"apps/web","exports":false,"externalDomains":[],"externalEnvVars":[],"internalDomains":[],"internalEnvVars":[],"main":false,"private":false,"serviceName":"web","workspace_dependents":[]},{"bin":false,"deploy_config":[],"directory":"services/api","exports":false,"externalDomains":[],"externalEnvVars":[],"internalDomains":[],"internalEnvVars":[],"main":false,"private":false,"serviceName":"api","workspace_dependents":[]}],"warnings":[]}],"repos_added":[],"repos_detected_by":"workspace manifest","repos_excluded":[],"schema":"carrick.derive/0","workspace":"<ROOT>"}"#;

/// Test 9, the other half: no project config and no carrick.json, and the
/// bytes are main's.
#[test]
fn derive_without_a_project_config_prints_the_bytes_it_did() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("mono");
    write(
        &root,
        "package.json",
        r#"{"name":"mono","private":true,"workspaces":["apps/*","services/*"]}"#,
    );
    write(
        &root,
        "apps/web/package.json",
        r#"{"name":"web","version":"1.0.0"}"#,
    );
    write(
        &root,
        "services/api/package.json",
        r#"{"name":"api","version":"1.0.0"}"#,
    );
    let output = carrick(
        &["derive", "--workspace", root.to_str().unwrap(), "--json"],
        None,
    )
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut text = String::from_utf8(output.stdout).unwrap();
    for spelling in [root.canonicalize().unwrap(), root.clone()] {
        text = text.replace(spelling.to_str().unwrap(), "<ROOT>");
    }
    assert_eq!(text.trim_end(), DERIVE_GOLDEN);
}
