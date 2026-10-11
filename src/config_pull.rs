//! `carrick config pull` (carrick#2233): the configuration Carrick holds for
//! this repository, written to a file a scan then reads through
//! `CARRICK_CONFIG`.
//!
//! The Action runs it before the install and the scan
//! (`scripts/pull-project-config.sh`), so a CI upload is written under the
//! same services as the cloud runner's scan of the same commit. Hidden from
//! `carrick --help`: nothing but that script calls it.
//!
//! Wire contract: the cloud's `project-config` action on
//! `/types/check-or-upload` (carrick-cloud
//! `lambdas/check-or-upload/project_config_action.ts`). The request carries
//! this tree's derived services and, when `carrick.json` is tracked at HEAD,
//! its committed body as a `seed`. The answer is a
//! `carrick.project-config/1` document, written to `--out` as it came.
//!
//! A refusal is never echoed: each code the action answers maps to a fixed
//! line here ([`PullError::line`]), so the cloud's error text stays internal.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};

use crate::oidc::OidcProvider;

/// The internal command: `carrick config pull ...`.
pub const COMMAND: &str = "config";

/// The schema of the answer, and of the file written.
pub const SCHEMA: &str = "carrick.project-config/1";

/// Attempts in all, the first included, before a transport error or a 5xx
/// becomes exit 1.
pub const ATTEMPTS: u32 = 3;

/// A committed `carrick.json` past this many bytes is not sent as a seed: the
/// action refuses a seed body over 300 KB (`SEED_MAX_BYTES` there).
pub const SEED_MAX_BYTES: usize = 300 * 1024;

/// Whether the argument list is `config pull ...`. Only both words: a bare
/// `carrick config` is a scan of a directory named `config`.
pub fn is_invocation(argv: &[String]) -> bool {
    matches!(argv, [command, sub, ..] if command == COMMAND && sub == "pull")
}

const USAGE: &str =
    "usage: carrick config pull --out <file> [--repo owner/name] [--workspace <dir>]";

/// What `carrick config pull` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullArgs {
    pub out: PathBuf,
    pub repo: Option<String>,
    pub workspace: PathBuf,
}

/// How the request authenticates: the Actions OIDC token, or the laptop's
/// credential. The same choice `CloudAuth::detect` makes, with the provider
/// handed in so a test can point it at a stub.
pub enum PullAuth<'a> {
    Oidc(&'a OidcProvider),
    Bearer(String),
}

/// A refusal the action answered, by its code. Never the cloud's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    InvalidRequest,
    SeedTooLarge,
    RepoMismatch,
    RepoNotConnected,
    /// 413: the answer is past what the API can return.
    TooLarge,
    /// Any other status the action or the gate in front of it refused with.
    Other(u16),
}

#[derive(Debug)]
pub enum PullError {
    NoCredential,
    NoRepo,
    Derive(String),
    Refused(Refusal),
    /// No usable answer after [`ATTEMPTS`] tries.
    Unanswered,
    /// An answer that is not a `carrick.project-config/1` document.
    Unreadable,
    Write(PathBuf, std::io::Error),
}

impl PullError {
    /// The one line this failure prints. Fixed text: nothing the server sent
    /// is part of it.
    pub fn line(&self) -> String {
        let text = match self {
            PullError::NoCredential => "no Carrick credential on this machine and no GitHub \
                                        Actions OIDC token to use."
                .to_string(),
            PullError::NoRepo => {
                "could not tell which repository this is; pass --repo owner/name.".to_string()
            }
            PullError::Derive(problem) => {
                format!("could not derive this repository's services: {problem}")
            }
            PullError::Refused(Refusal::InvalidRequest) => {
                "Carrick Cloud refused the request as malformed.".to_string()
            }
            PullError::Refused(Refusal::SeedTooLarge) => {
                "the committed carrick.json is too large to send.".to_string()
            }
            PullError::Refused(Refusal::RepoMismatch) => {
                "this run's token is for a different repository.".to_string()
            }
            PullError::Refused(Refusal::RepoNotConnected) => {
                "this repository is not connected to your Carrick workspace.".to_string()
            }
            PullError::Refused(Refusal::TooLarge) => {
                "the project's config is too large to download.".to_string()
            }
            PullError::Refused(Refusal::Other(status)) => {
                format!("Carrick Cloud refused the request (HTTP {status}).")
            }
            PullError::Unanswered => {
                format!("Carrick Cloud did not answer after {ATTEMPTS} tries.")
            }
            PullError::Unreadable => "Carrick Cloud's answer could not be read.".to_string(),
            PullError::Write(path, error) => {
                format!("could not write {}: {error}", path.display())
            }
        };
        format!("carrick config pull: {text}")
    }
}

/// Run the command. Exit 0 when the answer was written, `config: null`
/// included; 1 when nothing was written; 2 for a usage error.
pub async fn run(args: &[String]) -> i32 {
    let args = match parse(args) {
        Ok(args) => args,
        Err(problem) => {
            crate::errln!("carrick config: {problem}\n{USAGE}");
            return 2;
        }
    };
    let auth = match crate::credentials::CloudAuth::detect() {
        Ok(crate::credentials::CloudAuth::Bearer(token)) => PullAuth::Bearer(token),
        Ok(crate::credentials::CloudAuth::Oidc) => match OidcProvider::global() {
            Ok(provider) => PullAuth::Oidc(provider),
            Err(_) => return fail(PullError::NoCredential),
        },
        Err(_) => return fail(PullError::NoCredential),
    };
    let client = match crate::cloud_storage::http_client_builder().build() {
        Ok(client) => client,
        Err(_) => return fail(PullError::Unanswered),
    };
    let url = format!("{}/types/check-or-upload", env!("CARRICK_API_ENDPOINT"));
    match pull(&client, &url, &auth, &args, production_backoff).await {
        Ok(()) => 0,
        Err(error) => fail(error),
    }
}

fn fail(error: PullError) -> i32 {
    crate::errln!("{}", error.line());
    1
}

fn production_backoff(retries_so_far: u32) -> Duration {
    // 2s, then 4s.
    Duration::from_secs(2u64 << retries_so_far)
}

/// Read `pull --out <file> [--repo owner/name] [--workspace <dir>]`.
pub fn parse(args: &[String]) -> Result<PullArgs, String> {
    let Some((sub, rest)) = args.split_first() else {
        return Err("needs a subcommand".to_string());
    };
    if sub != "pull" {
        return Err(format!("unknown subcommand `{sub}`"));
    }
    let mut out = None;
    let mut repo = None;
    let mut workspace = None;
    let mut index = 0;
    while index < rest.len() {
        let flag = rest[index].as_str();
        let slot = match flag {
            "--out" => &mut out,
            "--repo" => &mut repo,
            "--workspace" | "-w" => &mut workspace,
            other => return Err(format!("unknown option `{other}`")),
        };
        index += 1;
        let value = rest
            .get(index)
            .filter(|value| !value.starts_with('-'))
            .ok_or_else(|| format!("{flag} needs a value"))?;
        *slot = Some(value.clone());
        index += 1;
    }
    Ok(PullArgs {
        out: PathBuf::from(out.ok_or("--out is required")?),
        repo,
        workspace: PathBuf::from(workspace.unwrap_or_else(|| ".".to_string())),
    })
}

/// Build the request, send it, and write the answer. Nothing is written
/// unless every step before the write succeeded.
pub async fn pull(
    client: &reqwest::Client,
    url: &str,
    auth: &PullAuth<'_>,
    args: &PullArgs,
    backoff: fn(u32) -> Duration,
) -> Result<(), PullError> {
    let repo = args
        .repo
        .clone()
        .or_else(|| crate::git_state::remote_name(&args.workspace))
        .ok_or(PullError::NoRepo)?;
    let body = request_body(&args.workspace, &repo).map_err(PullError::Derive)?;
    let answer = post(client, url, auth, &body, backoff).await?;
    write_answer(&args.out, &answer).map_err(|e| PullError::Write(args.out.clone(), e))
}

/// `{ action: "project-config", repos: [{ repo, derive, seed? }] }`.
pub fn request_body(root: &Path, repo: &str) -> Result<Value, String> {
    let mut entry = json!({ "repo": repo, "derive": derive_entry(root)? });
    if let Some(seed) = tracked_seed(root) {
        entry["seed"] = seed;
    }
    Ok(json!({ "action": "project-config", "repos": [entry] }))
}

/// This tree's services as `carrick derive --candidates` lists them: the
/// `repos[]` entry the action's planner reads (it reads `services` only).
fn derive_entry(root: &Path) -> Result<Value, String> {
    let derived = crate::service_derivation::resolve(root)?;
    let mut services = derived.service_documents();
    for (document, service) in services.iter_mut().zip(&derived.services) {
        let found = crate::derive_candidates::service_candidates(root, service);
        document["candidates"] =
            serde_json::to_value(found).expect("candidates serialize as a JSON object");
    }
    Ok(json!({ "services": services }))
}

/// The `carrick.json` committed at HEAD in `root`, as `{ commit, body }`.
///
/// `None` when it is not tracked, is over [`SEED_MAX_BYTES`], or is not a JSON
/// object: the action would refuse the whole request over such a seed, and a
/// request without one is still answered.
fn tracked_seed(root: &Path) -> Option<Value> {
    git(root, &["ls-files", "--error-unmatch", "--", "carrick.json"])?;
    let bytes = git(root, &["show", "HEAD:./carrick.json"])?;
    if bytes.len() > SEED_MAX_BYTES {
        return None;
    }
    let body: Value = serde_json::from_slice(&bytes).ok()?;
    if !body.is_object() {
        return None;
    }
    let commit = String::from_utf8(git(root, &["rev-parse", "HEAD"])?).ok()?;
    Some(json!({ "commit": commit.trim(), "body": body }))
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// POST the request, retrying a transport error or a 408/429/5xx until
/// [`ATTEMPTS`] are spent. An OIDC rejection re-mints once without counting.
async fn post(
    client: &reqwest::Client,
    url: &str,
    auth: &PullAuth<'_>,
    body: &Value,
    backoff: fn(u32) -> Duration,
) -> Result<Value, PullError> {
    let mut attempt = 0u32;
    let mut reminted = false;
    loop {
        let mut request = client
            .post(url)
            .header("X-Carrick-Scanner-Version", env!("CARGO_PKG_VERSION"))
            .json(body);
        let mut sent_token = None;
        request = match auth {
            PullAuth::Bearer(token) => request.header("Authorization", format!("Bearer {token}")),
            PullAuth::Oidc(provider) => {
                // Per attempt: the provider serves its cached token until it
                // nears expiry.
                let token = provider
                    .token()
                    .await
                    .map_err(|_| PullError::NoCredential)?;
                sent_token = Some(token.clone());
                request.header("X-Carrick-OIDC", token)
            }
        };
        attempt += 1;
        // A transport error falls through to the retry below, as does a
        // transient status; every other answer returns.
        if let Ok(response) = request.send().await {
            let status = response.status();
            let text = response.text().await.ok();
            if status.is_success() {
                return text
                    .ok_or(PullError::Unreadable)
                    .and_then(|t| read_answer(&t));
            }
            if let (PullAuth::Oidc(provider), Some(sent), Some(text)) =
                (auth, sent_token.as_deref(), text.as_deref())
                && !reminted
                && crate::agent_service::is_oidc_rejection(status.as_u16(), text)
            {
                reminted = true;
                attempt -= 1;
                provider
                    .remint(sent)
                    .await
                    .map_err(|_| PullError::NoCredential)?;
                continue;
            }
            if !crate::cloud_storage::is_transient_status(status) {
                return Err(PullError::Refused(refusal(
                    status,
                    text.as_deref().unwrap_or_default(),
                )));
            }
        }
        if attempt >= ATTEMPTS {
            return Err(PullError::Unanswered);
        }
        tokio::time::sleep(backoff(attempt - 1)).await;
    }
}

/// The refusal a non-transient answer names, by its `code` (or its status).
fn refusal(status: reqwest::StatusCode, text: &str) -> Refusal {
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        return Refusal::TooLarge;
    }
    let code = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|body| body.get("code")?.as_str().map(str::to_string));
    match code.as_deref() {
        Some("invalid_request") => Refusal::InvalidRequest,
        Some("seed_too_large") => Refusal::SeedTooLarge,
        Some("repo_mismatch") => Refusal::RepoMismatch,
        Some("repo_not_connected") => Refusal::RepoNotConnected,
        _ => Refusal::Other(status.as_u16()),
    }
}

/// The answer, when it is a `carrick.project-config/1` document.
fn read_answer(text: &str) -> Result<Value, PullError> {
    let answer: Value = serde_json::from_str(text).map_err(|_| PullError::Unreadable)?;
    if answer.get("schema").and_then(Value::as_str) != Some(SCHEMA)
        || !answer.get("projects").is_some_and(Value::is_array)
    {
        return Err(PullError::Unreadable);
    }
    Ok(answer)
}

/// Write the answer beside `out` and rename it into place, read-only, so a
/// reader sees the whole document or none.
fn write_answer(out: &Path, answer: &Value) -> std::io::Result<()> {
    let dir = match out.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&dir)?;
    let temp = dir.join(format!(".carrick-config-{}.tmp", std::process::id()));
    let mut text = serde_json::to_vec_pretty(answer).expect("a JSON value serializes");
    text.push(b'\n');
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&text)?;
        file.sync_all()?;
        set_read_only(&temp)?;
        std::fs::rename(&temp, out)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

#[cfg(unix)]
fn set_read_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444))
}

#[cfg(not(unix))]
fn set_read_only(path: &Path) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_service::tests::{stub_server, stub_token_endpoint};

    fn no_wait(_: u32) -> Duration {
        Duration::ZERO
    }

    fn client() -> reqwest::Client {
        crate::cloud_storage::http_client_builder()
            .no_proxy()
            .build()
            .unwrap()
    }

    fn sh(dir: &Path, args: &[&str]) {
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

    fn write(root: &Path, file: &str, body: &str) {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// `acme/mono`: npm workspaces `apps/web` and `services/api`, committed.
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
            "export const load = () => fetch(`${process.env.API_URL}/orders`);\n",
        );
        write(root, "services/api/package.json", r#"{ "name": "api" }"#);
        write(
            root,
            "services/api/src/server.ts",
            "import express from 'express';\nconst app = express();\napp.get('/orders', (_req, res) => res.json([]));\n",
        );
        sh(root, &["init", "-q"]);
        sh(
            root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/mono.git",
            ],
        );
        sh(root, &["add", "-A"]);
        sh(root, &["commit", "-qm", "init"]);
        dir
    }

    fn args(root: &Path, out: &Path) -> PullArgs {
        PullArgs {
            out: out.to_path_buf(),
            repo: None,
            workspace: root.to_path_buf(),
        }
    }

    fn answer(config: Value) -> String {
        json!({
            "schema": SCHEMA,
            "projects": [{ "id": "p1", "version": 3, "repos": [{
                "repo": "acme/mono", "github_repo_id": "7", "held": !config.is_null(),
                "behind": false, "config": config,
            }]}],
        })
        .to_string()
    }

    fn body_of(request: &str) -> Value {
        let (_, body) = request.split_once("\r\n\r\n").expect("request has a body");
        serde_json::from_str(body).expect("request body is JSON")
    }

    fn header(request: &str, name: &str) -> Option<String> {
        request.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
    }

    #[test]
    fn only_both_words_are_the_command() {
        let argv = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_invocation(&argv(&["config", "pull", "--out", "x"])));
        assert!(!is_invocation(&argv(&["config"])));
        assert!(!is_invocation(&argv(&["config", "--no-cache"])));
        assert!(!is_invocation(&argv(&["./config", "pull"])));
    }

    #[test]
    fn parse_reads_the_flags_and_requires_out() {
        let argv = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            parse(&argv(&[
                "pull",
                "--out",
                "/t/c.json",
                "--repo",
                "acme/mono",
                "--workspace",
                "sub"
            ])),
            Ok(PullArgs {
                out: "/t/c.json".into(),
                repo: Some("acme/mono".into()),
                workspace: "sub".into(),
            })
        );
        assert!(parse(&argv(&["pull"])).is_err());
        assert!(parse(&argv(&["pull", "--out"])).is_err());
        assert!(parse(&argv(&["pull", "--out", "--repo", "a/b"])).is_err());
        assert!(parse(&argv(&["push", "--out", "x"])).is_err());
        assert!(parse(&argv(&["pull", "--out", "x", "--verbose"])).is_err());
    }

    #[test]
    fn the_request_carries_the_derived_services_with_candidates_and_no_seed_when_nothing_is_tracked()
     {
        let repo = mono();
        // Untracked: scaffold, never a seed.
        write(
            repo.path(),
            "carrick.json",
            r#"{ "serviceName": "untracked" }"#,
        );
        let body = request_body(repo.path(), "acme/mono").unwrap();
        assert_eq!(body["action"], "project-config");
        let entry = &body["repos"][0];
        assert_eq!(entry["repo"], "acme/mono");
        assert!(entry.get("seed").is_none(), "{entry}");
        let services = entry["derive"]["services"].as_array().unwrap();
        assert!(
            services.iter().all(|s| s.get("candidates").is_some()),
            "{entry}"
        );
    }

    #[test]
    fn a_carrick_json_tracked_at_head_is_sent_as_the_seed() {
        let repo = mono();
        let committed = json!({ "serviceName": "mono", "internalEnvVars": ["API_URL"] });
        write(repo.path(), "carrick.json", &committed.to_string());
        sh(repo.path(), &["add", "carrick.json"]);
        sh(repo.path(), &["commit", "-qm", "config"]);
        // The working copy differs: the seed is what HEAD holds.
        std::fs::write(repo.path().join("carrick.json"), "{}").unwrap();
        let head = String::from_utf8(git(repo.path(), &["rev-parse", "HEAD"]).unwrap()).unwrap();

        let body = request_body(repo.path(), "acme/mono").unwrap();
        let seed = &body["repos"][0]["seed"];
        assert_eq!(seed["commit"], head.trim());
        assert_eq!(seed["body"], committed);
    }

    #[test]
    fn a_tracked_carrick_json_too_large_or_not_an_object_is_not_a_seed() {
        for body in [
            format!(
                r#"{{ "serviceName": "{}" }}"#,
                "x".repeat(SEED_MAX_BYTES + 1)
            ),
            "[1, 2]".to_string(),
            "{ not json".to_string(),
        ] {
            let repo = mono();
            write(repo.path(), "carrick.json", &body);
            sh(repo.path(), &["add", "carrick.json"]);
            sh(repo.path(), &["commit", "-qm", "config"]);
            assert_eq!(tracked_seed(repo.path()), None);
        }
    }

    #[tokio::test]
    async fn the_answer_is_written_read_only_as_it_came() {
        let repo = mono();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("carrick/config.json");
        let config = json!({ "services": [{ "serviceName": "web", "directory": "apps/web", "internalEnvVars": ["API_URL"] }] });
        let (url, server) = stub_server(vec![(200, answer(config.clone()))]);

        pull(
            &client(),
            &url,
            &PullAuth::Bearer("tok".into()),
            &args(repo.path(), &out),
            no_wait,
        )
        .await
        .unwrap();

        let requests = server.join().unwrap();
        assert_eq!(body_of(&requests[0])["repos"][0]["repo"], "acme/mono");
        let written: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(written["projects"][0]["repos"][0]["config"], config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&out).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o444);
        }
        // Nothing beside it: the temporary file was renamed, not left.
        let names: Vec<_> = std::fs::read_dir(out.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["config.json"]);
    }

    #[tokio::test]
    async fn a_second_pull_replaces_a_read_only_answer() {
        let repo = mono();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("config.json");
        let (url, server) = stub_server(vec![
            (200, answer(json!({ "serviceName": "first" }))),
            (200, answer(Value::Null)),
        ]);
        let auth = PullAuth::Bearer("tok".into());
        for _ in 0..2 {
            pull(&client(), &url, &auth, &args(repo.path(), &out), no_wait)
                .await
                .unwrap();
        }
        server.join().unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(written["projects"][0]["repos"][0]["config"], Value::Null);
    }

    #[tokio::test]
    async fn nothing_held_still_writes_the_answer_and_exits_zero() {
        let repo = mono();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("config.json");
        let (url, server) = stub_server(vec![(200, answer(Value::Null))]);
        pull(
            &client(),
            &url,
            &PullAuth::Bearer("tok".into()),
            &args(repo.path(), &out),
            no_wait,
        )
        .await
        .unwrap();
        server.join().unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(written["projects"][0]["repos"][0]["held"], false);
        assert_eq!(written["projects"][0]["repos"][0]["config"], Value::Null);
    }

    #[tokio::test]
    async fn a_server_error_on_every_try_writes_nothing_after_three_tries() {
        let repo = mono();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("config.json");
        let (url, server) = stub_server(vec![
            (500, r#"{"error":"internal detail"}"#.into()),
            (502, "bad gateway".into()),
            (503, "".into()),
        ]);
        let error = pull(
            &client(),
            &url,
            &PullAuth::Bearer("tok".into()),
            &args(repo.path(), &out),
            no_wait,
        )
        .await
        .unwrap_err();
        assert_eq!(server.join().unwrap().len(), ATTEMPTS as usize);
        assert!(matches!(error, PullError::Unanswered), "{error:?}");
        assert!(!out.exists());
        assert!(!out.parent().unwrap().read_dir().unwrap().any(|_| true));
    }

    #[tokio::test]
    async fn a_transient_error_then_an_answer_is_written() {
        let repo = mono();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("config.json");
        let (url, server) = stub_server(vec![
            (503, "".into()),
            (200, answer(json!({ "serviceName": "mono" }))),
        ]);
        pull(
            &client(),
            &url,
            &PullAuth::Bearer("tok".into()),
            &args(repo.path(), &out),
            no_wait,
        )
        .await
        .unwrap();
        assert_eq!(server.join().unwrap().len(), 2);
        assert!(out.exists());
    }

    #[tokio::test]
    async fn each_refusal_maps_to_a_fixed_line_that_never_carries_the_servers_text() {
        let secret = "SERVER-SIDE DETAIL";
        let cases = [
            (400, "invalid_request", Refusal::InvalidRequest),
            (400, "seed_too_large", Refusal::SeedTooLarge),
            (403, "repo_mismatch", Refusal::RepoMismatch),
            (403, "repo_not_connected", Refusal::RepoNotConnected),
            (413, "", Refusal::TooLarge),
            (409, "something_new", Refusal::Other(409)),
        ];
        let repo = mono();
        for (status, code, expected) in cases {
            let out_dir = tempfile::tempdir().unwrap();
            let out = out_dir.path().join("config.json");
            let body = json!({ "error": secret, "code": code }).to_string();
            let (url, server) = stub_server(vec![(status, body)]);
            let error = pull(
                &client(),
                &url,
                &PullAuth::Bearer("tok".into()),
                &args(repo.path(), &out),
                no_wait,
            )
            .await
            .unwrap_err();
            assert_eq!(server.join().unwrap().len(), 1, "a refusal is not retried");
            match &error {
                PullError::Refused(refusal) => assert_eq!(refusal, &expected),
                other => panic!("{status} {code}: {other:?}"),
            }
            let line = error.line();
            assert!(!line.contains(secret), "{line}");
            assert!(!line.contains(code) || code.is_empty(), "{line}");
            assert!(!out.exists());
        }
    }

    #[tokio::test]
    async fn an_answer_that_is_not_a_project_config_document_writes_nothing() {
        let repo = mono();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("config.json");
        let (url, server) =
            stub_server(vec![(200, r#"{"schema":"other/1","projects":[]}"#.into())]);
        let error = pull(
            &client(),
            &url,
            &PullAuth::Bearer("tok".into()),
            &args(repo.path(), &out),
            no_wait,
        )
        .await
        .unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, PullError::Unreadable), "{error:?}");
        assert!(!out.exists());
    }

    #[tokio::test]
    async fn in_actions_the_request_carries_the_oidc_token_and_no_bearer_credential() {
        let repo = mono();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("config.json");
        let token = crate::oidc::tests::jwt_with_exp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 3600,
        );
        let (token_url, token_server) = stub_token_endpoint(vec![token.clone()]);
        let provider = OidcProvider::for_test(token_url, "request-token".into());
        let (url, server) = stub_server(vec![(200, answer(Value::Null))]);
        pull(
            &client(),
            &url,
            &PullAuth::Oidc(&provider),
            &args(repo.path(), &out),
            no_wait,
        )
        .await
        .unwrap();
        token_server.join().unwrap();
        let requests = server.join().unwrap();
        assert_eq!(header(&requests[0], "x-carrick-oidc"), Some(token));
        assert_eq!(header(&requests[0], "authorization"), None);
    }

    #[tokio::test]
    async fn the_repo_is_the_origin_remote_unless_one_is_passed() {
        let repo = mono();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("config.json");
        let (url, server) = stub_server(vec![(200, answer(Value::Null))]);
        let mut named = args(repo.path(), &out);
        named.repo = Some("acme/Other".into());
        pull(
            &client(),
            &url,
            &PullAuth::Bearer("t".into()),
            &named,
            no_wait,
        )
        .await
        .unwrap();
        let requests = server.join().unwrap();
        assert_eq!(body_of(&requests[0])["repos"][0]["repo"], "acme/Other");
    }

    #[tokio::test]
    async fn a_tree_with_no_remote_and_no_repo_named_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("config.json");
        let error = pull(
            &client(),
            "http://127.0.0.1:1",
            &PullAuth::Bearer("t".into()),
            &args(dir.path(), &out),
            no_wait,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, PullError::NoRepo), "{error:?}");
        assert!(!out.exists());
    }
}
