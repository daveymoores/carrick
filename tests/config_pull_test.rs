//! carrick#2233: `carrick config pull` and the Action step that runs it.
//!
//! The request, the seed, the refusals, OIDC and the atomic write are unit
//! tests beside the code (`src/config_pull.rs`), against a loopback stub: the
//! binary's endpoint is fixed at build time. These cases drive what the
//! binary and the Action do with the command:
//!
//! - the step script (`scripts/pull-project-config.sh`) on its two exits;
//! - `action.yml`: every Carrick step after the pull is gated on it;
//! - the binary: hidden from help, usage, and a failure that writes nothing;
//! - the scan under the pulled document (marked `ignore` until the scan reads
//!   `CARRICK_CONFIG`, carrick#2272, which ships in the same release).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn carrick() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_carrick"));
    for var in [
        "CARRICK_TOKEN",
        "CARRICK_CONFIG",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
        "GITHUB_REPOSITORY",
        "GITHUB_ACTIONS",
        "CI",
    ] {
        command.env_remove(var);
    }
    command
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn write(root: &Path, file: &str, body: &str) {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// `acme/mono`: npm workspaces `apps/web` and `services/api`. `apps/web`
/// calls `${process.env.API_URL}/orders`; `services/api` serves `/orders`.
fn mono() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "package.json",
        r#"{ "name": "mono", "private": true, "workspaces": ["apps/*", "services/*"] }"#,
    );
    write(root, "apps/web/package.json", r#"{ "name": "web" }"#);
    write(
        root,
        "apps/web/src/orders.ts",
        "export async function loadOrders() {\n  const res = await fetch(`${process.env.API_URL}/orders`);\n  return res.json();\n}\n",
    );
    write(root, "services/api/package.json", r#"{ "name": "api" }"#);
    write(
        root,
        "services/api/src/server.ts",
        "import express from 'express';\nconst app = express();\napp.get('/orders', (_req, res) => { res.json([]); });\napp.listen(3000);\n",
    );
    git(root, &["init", "-q"]);
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
    git(root, &["commit", "-qm", "init"]);
    dir
}

// ---------------------------------------------------------------- the step

struct Step {
    status: i32,
    said: String,
    github_env: String,
    github_output: String,
    runner_temp: tempfile::TempDir,
}

/// Run the step script with `cli` as the resolved Carrick.
fn step(cli: &Path, scan_path: &Path, extra_env: &[(&str, &str)]) -> Step {
    let runner_temp = tempfile::tempdir().unwrap();
    let github_env = runner_temp.path().join("github_env");
    let github_output = runner_temp.path().join("github_output");
    let mut command = Command::new("bash");
    command
        .arg(repo_root().join("scripts/pull-project-config.sh"))
        .arg(scan_path)
        .env("CARRICK_CLI", cli)
        .env("RUNNER_TEMP", runner_temp.path())
        .env("GITHUB_ENV", &github_env)
        .env("GITHUB_OUTPUT", &github_output)
        .env_remove("CARRICK_TOKEN")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    Step {
        status: output.status.code().unwrap_or(-1),
        said: text(&output),
        github_env: std::fs::read_to_string(&github_env).unwrap_or_default(),
        github_output: std::fs::read_to_string(&github_output).unwrap_or_default(),
        runner_temp,
    }
}

/// A stand-in Carrick: records its arguments and whether `CARRICK_CONFIG`
/// reached it, prints a line on each stream, and exits `code` (writing
/// `--out` first on 0).
fn fake_cli(dir: &Path, code: i32) -> PathBuf {
    let path = dir.join("fake-carrick");
    write(
        dir,
        "fake-carrick",
        &format!(
            r#"#!/usr/bin/env bash
printf '%s\n' "$@" > "{record}"
echo "inherited=${{CARRICK_CONFIG:-}}" >> "{record}"
echo "said on stdout"
echo "said on stderr" >&2
if [ {code} -eq 0 ]; then
  while [ $# -gt 0 ]; do
    if [ "$1" = "--out" ]; then echo '{{}}' > "$2"; fi
    shift
  done
fi
exit {code}
"#,
            record = dir.join("record").display(),
        ),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

#[test]
fn a_pull_that_writes_its_answer_sets_carrick_config_and_says_nothing() {
    let bin = tempfile::tempdir().unwrap();
    let cli = fake_cli(bin.path(), 0);
    let scan = tempfile::tempdir().unwrap();
    let run = step(
        &cli,
        scan.path(),
        &[("CARRICK_CONFIG", "/stale/config.json")],
    );

    assert_eq!(run.status, 0);
    assert_eq!(run.said, "", "the step prints nothing");
    let out = run.runner_temp.path().join("carrick/config.json");
    assert_eq!(
        run.github_env,
        format!("CARRICK_CONFIG={}\n", out.display())
    );
    assert_eq!(run.github_output, "skip=false\n");
    let record = std::fs::read_to_string(bin.path().join("record")).unwrap();
    assert_eq!(
        record,
        format!(
            "config\npull\n--workspace\n{}\n--out\n{}\ninherited=\n",
            scan.path().display(),
            out.display()
        ),
        "the pull runs on the scan path, and an inherited CARRICK_CONFIG never reaches its derive"
    );
}

#[test]
fn a_pull_that_fails_skips_every_later_step_keeps_the_job_green_and_says_nothing() {
    let bin = tempfile::tempdir().unwrap();
    let cli = fake_cli(bin.path(), 1);
    let scan = tempfile::tempdir().unwrap();
    let run = step(&cli, scan.path(), &[]);

    assert_eq!(run.status, 0, "the job stays green");
    assert_eq!(run.said, "", "the step prints nothing");
    assert_eq!(run.github_env, "", "no CARRICK_CONFIG for later steps");
    assert_eq!(run.github_output, "skip=true\n");
}

#[test]
fn the_real_binary_without_a_credential_skips_quietly() {
    let scan = mono();
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().to_str().unwrap();
    let run = step(
        Path::new(env!("CARGO_BIN_EXE_carrick")),
        scan.path(),
        &[("HOME", home_path), ("XDG_CONFIG_HOME", home_path)],
    );
    assert_eq!(run.status, 0);
    assert_eq!(run.said, "");
    assert_eq!(run.github_output, "skip=true\n");
    assert!(
        !run.runner_temp.path().join("carrick/config.json").exists(),
        "nothing written"
    );
}

/// The steps of `action.yml`, in order.
fn action_steps() -> Vec<serde_yaml_ng::Value> {
    let text = std::fs::read_to_string(repo_root().join("action.yml")).unwrap();
    let action: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).unwrap();
    action["runs"]["steps"]
        .as_sequence()
        .expect("runs.steps")
        .clone()
}

fn condition(step: &serde_yaml_ng::Value) -> String {
    step["if"].as_str().unwrap_or_default().to_string()
}

#[test]
fn every_carrick_step_after_the_pull_is_gated_on_it() {
    let steps = action_steps();
    let pull = steps
        .iter()
        .position(|s| s["id"].as_str() == Some("config"))
        .expect("action.yml has the project-config step");
    let cli = steps
        .iter()
        .position(|s| s["id"].as_str() == Some("cli"))
        .expect("action.yml names the resolved Carrick");
    let deps = steps
        .iter()
        .position(|s| s["id"].as_str() == Some("deps"))
        .expect("action.yml detects the install");

    assert!(
        cli < pull && pull < deps,
        "the pull runs after the wrapper is named and before the install"
    );
    // The pull needs the wrapper whatever `install-dependencies` says.
    for index in [cli, pull] {
        assert!(
            !condition(&steps[index]).contains("install-dependencies"),
            "{}",
            condition(&steps[index])
        );
    }
    assert!(
        steps[pull]["run"]
            .as_str()
            .unwrap()
            .contains("scripts/pull-project-config.sh\" \"$SCAN_PATH\"")
    );
    assert_eq!(
        steps[pull]["env"]["SCAN_PATH"].as_str(),
        Some("${{ inputs.path }}")
    );
    for step in &steps[pull + 1..] {
        assert!(
            condition(step).contains("steps.config.outputs.skip != 'true'"),
            "step `{}` runs after a failed pull: {}",
            step["name"].as_str().unwrap_or_default(),
            condition(step)
        );
    }
}

// -------------------------------------------------------------- the binary

#[test]
fn the_command_is_not_named_in_help() {
    let output = carrick().arg("--help").output().unwrap();
    assert!(!text(&output).contains("config pull"), "{}", text(&output));
}

#[test]
fn a_pull_without_out_is_a_usage_error() {
    let output = carrick().args(["config", "pull"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
}

#[test]
fn a_pull_with_no_credential_exits_one_with_a_fixed_line_and_writes_nothing() {
    let scan = mono();
    let home = tempfile::tempdir().unwrap();
    let out = home.path().join("out/config.json");
    let output = carrick()
        .args(["config", "pull", "--workspace"])
        .arg(scan.path())
        .arg("--out")
        .arg(&out)
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "carrick config pull: no Carrick credential on this machine and no GitHub Actions \
         OIDC token to use.\n"
    );
    assert!(!out.exists());
}

// ------------------------------------------- the scan under the pulled file

/// A loopback `project-config` answer, recording the request it was sent.
fn serve_once(answer: String) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut raw = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = stream.read(&mut buf).unwrap();
            raw.extend_from_slice(&buf[..n]);
            let seen = String::from_utf8_lossy(&raw).to_string();
            if let Some(end) = seen.find("\r\n\r\n") {
                let length = seen[..end]
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if raw.len() >= end + 4 + length {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            answer.len(),
            answer
        );
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8_lossy(&raw).to_string()
    });
    (format!("http://{addr}/types/check-or-upload"), handle)
}

/// The document Carrick holds for `acme/mono`: both members, with `API_URL`
/// labelled internal on `web`. A scan that inferred instead has no label.
fn held_document() -> Value {
    json!({
        "schema": carrick::config_pull::SCHEMA,
        "projects": [{ "id": "p1", "version": 2, "repos": [{
            "repo": "acme/mono", "github_repo_id": "7", "held": true, "behind": false,
            "config": { "services": [
                { "serviceName": "api", "directory": "services/api" },
                { "serviceName": "web", "directory": "apps/web", "internalEnvVars": ["API_URL"] },
            ]},
        }]}],
    })
}

/// Pull through the library against a loopback answer, as the binary does
/// against the cloud.
fn pull_into(root: &Path, out: &Path, answer: Value) -> Value {
    let (url, server) = serve_once(answer.to_string());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    runtime
        .block_on(carrick::config_pull::pull(
            &client,
            &url,
            &carrick::config_pull::PullAuth::Bearer("t".into()),
            &carrick::config_pull::PullArgs {
                out: out.to_path_buf(),
                repo: None,
                workspace: root.to_path_buf(),
            },
            |_| std::time::Duration::ZERO,
        ))
        .unwrap();
    let request = server.join().unwrap();
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

/// Scan `root` under `CARRICK_CONFIG=config` with the model off; each stored
/// service's name and `config_json`.
fn scan_services(root: &Path, config: Option<&Path>) -> Vec<(String, Value)> {
    let storage = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let mut command = carrick();
    command
        .arg(root)
        .env("CARRICK_MOCK_ALL", "1")
        .env("CARRICK_NO_MODEL", "1")
        .env("CARRICK_SKIP_INTENTS", "1")
        .env("CARRICK_CACHE_DIR", cache.path())
        .env("CARRICK_LOCAL_STORAGE_DIR", storage.path())
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_ALLOW_UNPREPARED", "1");
    if let Some(config) = config {
        command.env("CARRICK_CONFIG", config);
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "scan failed:\n{}", text(&output));
    let mut services: Vec<(String, Value)> = std::fs::read_dir(storage.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).ok())
        .filter(|blob| blob.get("function_definitions").is_some())
        .map(|blob| {
            (
                blob["service_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                serde_json::from_str(blob["config_json"].as_str().unwrap_or("null")).unwrap(),
            )
        })
        .collect();
    services.sort_by(|a, b| a.0.cmp(&b.0));
    services
}

/// Test 1: CI (pull, then scan) and the runner (the same document handed
/// straight to the scan) store the same services. The break test: skip the
/// pull, and `web` has no `internalEnvVars`.
#[test]
#[ignore = "waits on carrick#2272: the scan reads CARRICK_CONFIG"]
fn ci_and_the_runner_store_the_same_services() {
    let repo = mono();
    let temp = tempfile::tempdir().unwrap();
    let pulled = temp.path().join("pulled.json");
    pull_into(repo.path(), &pulled, held_document());
    let handed = temp.path().join("handed.json");
    std::fs::write(&handed, held_document().to_string()).unwrap();

    let ci = scan_services(repo.path(), Some(&pulled));
    let runner = scan_services(repo.path(), Some(&handed));
    assert_eq!(ci, runner);
    let web = ci
        .iter()
        .find(|(name, _)| name == "web")
        .expect("web stored");
    assert_eq!(web.1["internalEnvVars"], json!(["API_URL"]));
    // The checkout is untouched: the file lives outside it.
    let status = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&status.stdout), "");
}

/// Test 2: a tracked carrick.json is sent as the seed, and the scan uses the
/// answer, never the file.
#[test]
#[ignore = "waits on carrick#2272: the scan reads CARRICK_CONFIG, never carrick.json"]
fn a_tracked_carrick_json_is_the_seed_and_the_scan_uses_the_answer() {
    let repo = mono();
    write(
        repo.path(),
        "carrick.json",
        r#"{ "serviceName": "whole-repo" }"#,
    );
    git(repo.path(), &["add", "carrick.json"]);
    git(repo.path(), &["commit", "-qm", "config"]);
    let temp = tempfile::tempdir().unwrap();
    let pulled = temp.path().join("pulled.json");
    let request = pull_into(repo.path(), &pulled, held_document());
    assert_eq!(
        request["repos"][0]["seed"]["body"],
        json!({ "serviceName": "whole-repo" })
    );
    let names: Vec<String> = scan_services(repo.path(), Some(&pulled))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["api", "web"]);
}

/// Test 3: nothing held and nothing tracked: the scan derives, so its
/// services are `carrick derive`'s.
#[test]
fn nothing_held_scans_what_derive_names() {
    let repo = mono();
    let temp = tempfile::tempdir().unwrap();
    let pulled = temp.path().join("pulled.json");
    let mut answer = held_document();
    answer["projects"][0]["repos"][0]["held"] = json!(false);
    answer["projects"][0]["repos"][0]["config"] = Value::Null;
    let request = pull_into(repo.path(), &pulled, answer);
    assert!(request["repos"][0].get("seed").is_none());

    let derived = carrick()
        .args(["derive", "--workspace"])
        .arg(repo.path())
        .output()
        .unwrap();
    let derived: Value = serde_json::from_slice(&derived.stdout).unwrap();
    let mut derived_names: Vec<String> = derived["repos"][0]["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["serviceName"].as_str().unwrap().to_string())
        .collect();
    derived_names.sort();
    let scanned: Vec<String> = scan_services(repo.path(), Some(&pulled))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(scanned, derived_names);
}

/// Test 6: the install prepares the pulled document's services.
#[test]
#[ignore = "waits on carrick#2272: `carrick derive` reads CARRICK_CONFIG"]
fn the_install_prepares_the_pulled_services() {
    let repo = mono();
    // Each member states an install of its own.
    for (directory, name) in [("apps/web", "web"), ("services/api", "api")] {
        write(
            repo.path(),
            &format!("{directory}/package.json"),
            &format!(r#"{{"name":"{name}","dependencies":{{"left-pad":"1.3.0"}}}}"#),
        );
        write(repo.path(), &format!("{directory}/package-lock.json"), "{}");
    }
    // A document that keeps only `api`: the install must not prepare `web`.
    let mut answer = held_document();
    answer["projects"][0]["repos"][0]["config"] =
        json!({ "services": [{ "serviceName": "api", "directory": "services/api" }] });
    let temp = tempfile::tempdir().unwrap();
    let pulled = temp.path().join("pulled.json");
    pull_into(repo.path(), &pulled, answer);
    let output = Command::new("bash")
        .arg(repo_root().join("scripts/install-scanned-deps.sh"))
        .arg("detect")
        .arg(repo.path())
        .env("CARRICK_CLI", env!("CARGO_BIN_EXE_carrick"))
        .env("CARRICK_CONFIG", &pulled)
        .output()
        .unwrap();
    // `detect` logs each root it prepares as "Carrick prepares <root> with".
    let said = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        said.contains("Carrick prepares services/api with"),
        "{said}"
    );
    assert!(!said.contains("Carrick prepares apps/web with"), "{said}");
}
