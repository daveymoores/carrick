//! Live cross-repo accuracy scorer (cross-repo eval S4 thin slice, #203).
//!
//! Scores the *real* scanner over the authored `xrepo-corpus-1` constellation and
//! reports two metric families, **report-only** (a monitor, never a gate):
//!
//!   1. **endpoint-set P/R/F1** (corpus-wide) — every repo's
//!      `expected.json.endpoints` unioned, vs the joined [`EvalProjection`]'s
//!      `endpoints`, compared as a set of normalized `(METHOD, norm_path(path))`
//!      (contract §6 row 1). Scored corpus-wide rather than per repo because the
//!      joined projection carries no per-repo provenance on `endpoints` yet;
//!      a per-repo breakdown is a TODO (see below).
//!   2. **cross-repo match P/R/F1** — `expected-output.json.matches` vs
//!      `EvalProjection.cross_repo_matches`, keyed by
//!      `(producer_repo, norm(producer_key), consumer_repo, norm(consumer_key))`
//!      (contract §6 row 7).
//!
//! N runs (default 5, `CARRICK_EVAL_RUNS`), reported as **mean ± sample sd** per
//! metric, and one [`EvalRunRecord`] JSONL row (`tier="xrepo"`,
//! `corpus="xrepo-corpus-1"`) is written + echoed for the Axiom history.
//!
//! **DEFERRED** (not in this slice — clear TODO markers below, citing the S0
//! scorer contract §6 rows):
//!   - type-anchor accuracy (row 5),
//!   - type-resolution correctness (row 6),
//!   - compat-verdict accuracy (row 8) + the §7 `ts_check_dir` guard's *scoring*
//!     half (the dir is still wired in Phase B so the seam is live),
//!   - dependency conflicts (row 9),
//!   - negative `_must_not_emit` decoy leakage (row 3),
//!   - owner accuracy (row 4),
//!   - orphans (row 10),
//!   - capability-vs-roadmap partitioning (every corpus label is `capability`
//!     today, so partitioning is a no-op; wire it when `roadmap` labels exist).
//!   - the full monitor cadence (debounced-on-main + issue-filing) — see build
//!     plan §7 slice 4; this thin slice ships `workflow_dispatch` only.
//!
//! ## LIVE vs offline
//! The scored run uses the **real LLM** (no `CARRICK_MOCK_ALL`), so it only runs
//! in CI with GitHub Actions OIDC granting the scanner its keyless cloud auth. It
//! reuses the S2 two-phase `LocalDirStorage` harness (Phase A isolated per repo,
//! Phase B joins) — but in LIVE mode, with a *distinct* env policy: see
//! [`strip_ci_identity_keep_oidc`]. Gated behind `#[ignore]` + `CARRICK_EVAL_LIVE`
//! so plain `cargo test` never triggers a costly scan; the scoring-math is covered
//! by the cheap `#[cfg(test)]`-style unit tests that DO run normally.
//!
//! Runner: `.github/workflows/eval-xrepo.yml` (`workflow_dispatch`).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CORPUS: &str = "xrepo-corpus-1";
const DEFAULT_RUNS: usize = 5;

// ---------------------------------------------------------------------------
// Shared primitives (contract §1) — mirrors of the Tier-A scorer's helpers.
// Kept local because integration tests are separate crates and cannot share a
// private test module; these are intentionally identical to `eval_tier_a.rs`.
// ---------------------------------------------------------------------------

/// Collapse `:x` / `{x}` / `[x]` param syntaxes to `:param` and strip a single
/// trailing slash (contract §1.1). This is what makes the `:id`/`{id}`/`[id]`
/// corpus traps (#167) score as matches rather than spurious mismatches.
fn norm_path(p: &str) -> String {
    let mut out = String::new();
    for (i, seg) in p.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        let is_param = seg.starts_with(':')
            || (seg.starts_with('{') && seg.ends_with('}'))
            || (seg.starts_with('[') && seg.ends_with(']'));
        out.push_str(if is_param { ":param" } else { seg });
    }
    if out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// Normalize a canonical operation key `"<protocol>|<METHOD>|<path>"` (contract
/// §1.2) by running its path segment through [`norm_path`]. A key without the two
/// pipes is returned unchanged (defensive; the corpus always supplies the full
/// form).
fn norm_key(key: &str) -> String {
    let mut parts = key.splitn(3, '|');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(proto), Some(method), Some(path)) => {
            format!("{proto}|{}|{}", method.to_uppercase(), norm_path(path))
        }
        _ => key.to_string(),
    }
}

/// P/R/F1 with the contract §1.4 convention: precision is 1.0 when nothing is
/// expected and nothing is found, 0.0 when something is found but nothing matched
/// the empty expectation; recall is 1.0 when nothing is expected.
fn prf(tp: usize, found: usize, expected: usize) -> (f64, f64, f64) {
    let precision = if found == 0 {
        if expected == 0 { 1.0 } else { 0.0 }
    } else {
        tp as f64 / found as f64
    };
    let recall = if expected == 0 {
        1.0
    } else {
        tp as f64 / expected as f64
    };
    let f1 = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };
    (precision, recall, f1)
}

/// Mean and sample stddev (n-1). stddev is 0.0 for n < 2 (contract §1.4).
fn mean_sd(xs: &[f64]) -> (f64, f64) {
    let n = xs.len();
    if n == 0 {
        return (0.0, 0.0);
    }
    let mean = xs.iter().sum::<f64>() / n as f64;
    if n < 2 {
        return (mean, 0.0);
    }
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0);
    (mean, var.sqrt())
}

// ---------------------------------------------------------------------------
// Label shapes (contract §5) — only the fields THIS slice scores are read.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ExpectedRepo {
    #[serde(default)]
    endpoints: Vec<ExpEndpoint>,
    // DEFERRED: `calls` (row 2), `_must_not_emit` (row 3) — not scored here.
}

#[derive(Debug, Deserialize)]
struct ExpEndpoint {
    method: String,
    path: String,
    // DEFERRED: `owner` (row 4), `primary_type_symbol` (row 5),
    // `resolved_type`/`type_state` (row 6), `tier` (partitioning) — not read here.
}

#[derive(Debug, Deserialize)]
struct ExpectedOutput {
    #[serde(default)]
    matches: Vec<ExpMatch>,
    // DEFERRED: `orphans` (row 10), `dependency_conflicts` (row 9) — not read here.
}

#[derive(Debug, Deserialize)]
struct ExpMatch {
    producer_repo: String,
    producer_key: String,
    consumer_repo: String,
    consumer_key: String,
    // DEFERRED: `type_compatible`/`mismatch_reason` (row 8), `tier`
    // (partitioning) — not read here.
}

// ---------------------------------------------------------------------------
// EvalProjection wire shape. Mirrored locally (not the lib type) so the harness
// reads the projection purely as a wire contract — same stance as the S2 harness.
// Only the fields this slice scores are deserialized.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EvalProjection {
    #[serde(default)]
    endpoints: Vec<EvalOp>,
    #[serde(default)]
    calls: Vec<EvalOp>,
    #[serde(default)]
    cross_repo_matches: Vec<EvalCrossRepoMatch>,
    // `dependency_conflicts` and the per-op type fields exist on the wire (S1)
    // but are DEFERRED by this slice. `calls` is read for the extraction
    // diagnostic only (not yet scored — call-set is a deferred metric).
}

#[derive(Debug, Deserialize)]
struct EvalOp {
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EvalCrossRepoMatch {
    producer_repo: String,
    producer_key: String,
    consumer_repo: String,
    consumer_key: String,
    // `type_compatible`/`mismatch_reason` exist on the wire but compat scoring
    // (row 8) is DEFERRED.
}

// ---------------------------------------------------------------------------
// Pure scoring functions (unit-tested below with synthetic projections).
// ---------------------------------------------------------------------------

/// Endpoint-set P/R/F1 for ONE repo (contract §6 row 1): the set of normalized
/// `(METHOD, norm_path(path))` from the expected endpoints vs the projection's
/// HTTP endpoints. `expected_repo_endpoints` is the slice of labels for that one
/// repo; `proj_endpoints` is filtered to that repo's emissions by the caller (the
/// joined projection is per-corpus, so the live path keys endpoints to a repo via
/// the producer-side match edges — see [`score_corpus`]).
fn score_endpoint_set(
    expected: &[(String, String)],
    found: &HashSet<(String, String)>,
) -> (f64, f64, f64) {
    let expected_set: HashSet<(String, String)> = expected.iter().cloned().collect();
    let tp = expected_set.intersection(found).count();
    prf(tp, found.len(), expected_set.len())
}

/// The edge key for the cross-repo match metric (contract §6 row 7):
/// `(producer_repo, norm(producer_key), consumer_repo, norm(consumer_key))`.
fn match_edge_key(
    producer_repo: &str,
    producer_key: &str,
    consumer_repo: &str,
    consumer_key: &str,
) -> (String, String, String, String) {
    (
        producer_repo.to_string(),
        norm_key(producer_key),
        consumer_repo.to_string(),
        norm_key(consumer_key),
    )
}

/// Cross-repo match P/R/F1 (contract §6 row 7): exact set equality over the edge
/// key, both sides normalized.
fn score_matches(expected: &[ExpMatch], found: &[EvalCrossRepoMatch]) -> (f64, f64, f64) {
    let expected_set: HashSet<(String, String, String, String)> = expected
        .iter()
        .map(|m| {
            match_edge_key(
                &m.producer_repo,
                &m.producer_key,
                &m.consumer_repo,
                &m.consumer_key,
            )
        })
        .collect();
    let found_set: HashSet<(String, String, String, String)> = found
        .iter()
        .map(|m| {
            match_edge_key(
                &m.producer_repo,
                &m.producer_key,
                &m.consumer_repo,
                &m.consumer_key,
            )
        })
        .collect();
    let tp = expected_set.intersection(&found_set).count();
    prf(tp, found_set.len(), expected_set.len())
}

/// The full corpus's expected endpoint set: every repo's `expected.json`
/// endpoints, normalized and tagged by repo. The joined projection has no
/// per-repo provenance on `endpoints`, so endpoint-set scoring is done over the
/// **corpus-wide** set (the union of all repos' labels vs all projected
/// endpoints) — a single P/R/F1 for the whole constellation. This is the honest
/// thin-slice reading of row 1: per-repo partitioning of the joined endpoints
/// needs repo provenance on `EvalOp`, which S1 did not add (TODO: thread repo id
/// onto projected endpoints to restore the per-repo breakdown).
fn corpus_expected_endpoints(repo_expected: &[(String, ExpectedRepo)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (_repo, exp) in repo_expected {
        for e in &exp.endpoints {
            out.push((e.method.to_uppercase(), norm_path(&e.path)));
        }
    }
    out
}

/// The projection's HTTP endpoints as a normalized `(METHOD, path)` set.
fn projection_endpoint_set(proj: &EvalProjection) -> HashSet<(String, String)> {
    proj.endpoints
        .iter()
        .filter(|o| o.protocol == "http")
        .filter_map(|o| {
            Some((
                o.method.clone()?.to_uppercase(),
                norm_path(o.path.as_deref()?),
            ))
        })
        .collect()
}

/// One run's two metric vectors.
struct RunScore {
    ep_prf: (f64, f64, f64),
    match_prf: (f64, f64, f64),
}

/// Score one joined projection against the corpus labels.
fn score_corpus(
    proj: &EvalProjection,
    repo_expected: &[(String, ExpectedRepo)],
    expected_output: &ExpectedOutput,
) -> RunScore {
    let expected_eps = corpus_expected_endpoints(repo_expected);
    let found_eps = projection_endpoint_set(proj);
    let ep_prf = score_endpoint_set(&expected_eps, &found_eps);
    let match_prf = score_matches(&expected_output.matches, &proj.cross_repo_matches);
    RunScore { ep_prf, match_prf }
}

// ---------------------------------------------------------------------------
// EvalRunRecord — the longitudinal store row. Same schema as Tier-A's record
// (the cross-repo fields were added S6); here we populate `tier`/`corpus` +
// `match_*` and reuse `ep_*` for the endpoint-set metric.
// ---------------------------------------------------------------------------

const RECORD_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvalRunRecord {
    schema_version: u32,
    ts_unix: u64,
    scanner_version: String,
    carrick_sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github_run_id: Option<String>,
    fixture: String,
    runs_requested: usize,
    runs_effective: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_hash: Option<String>,
    // Endpoint-set metric (contract §6 row 1) rides the `ep_*` columns.
    ep_precision_mean: f64,
    ep_precision_sd: f64,
    ep_recall_mean: f64,
    ep_recall_sd: f64,
    ep_f1_mean: f64,
    ep_f1_sd: f64,
    ep_pass_at_k: f64,
    ep_pass_pow_k: f64,
    // Call metrics are not scored by this slice; emitted as a perfect-empty 0/0.
    call_precision_mean: f64,
    call_precision_sd: f64,
    call_recall_mean: f64,
    call_recall_sd: f64,
    call_f1_mean: f64,
    call_f1_sd: f64,
    call_pass_at_k: f64,
    call_pass_pow_k: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    // --- cross-repo facets (the half this slice fills) ---
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    corpus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    match_precision_mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    match_precision_sd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    match_recall_mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    match_recall_sd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    match_f1_mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    match_f1_sd: Option<f64>,
    // DEFERRED cross-repo facets (contract §6 rows 4-6, 8-10): left None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    type_anchor_accuracy_mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    type_anchor_accuracy_sd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    type_resolution_accuracy_mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    type_resolution_accuracy_sd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compat_verdict_accuracy_mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compat_verdict_accuracy_sd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_accuracy_mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_accuracy_sd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decoy_leak_mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decoy_leak_sd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dep_f1_mean: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dep_f1_sd: Option<f64>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn corpus_dir() -> PathBuf {
    manifest_dir().join("tests/fixtures").join(CORPUS)
}

/// The corpus repos: every immediate subdirectory of the corpus dir that holds a
/// `package.json`. Crucially this lists the **3 top-level repos** and does NOT
/// descend into `orders-monorepo/packages/*` (those package.jsons are not
/// immediate children of the corpus dir). Sorted for deterministic Phase-A order.
fn discover_repos(corpus: &Path) -> Vec<PathBuf> {
    let mut repos: Vec<PathBuf> = std::fs::read_dir(corpus)
        .unwrap_or_else(|e| panic!("failed to read corpus dir {}: {e}", corpus.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join("package.json").exists())
        .collect();
    repos.sort();
    repos
}

// ---------------------------------------------------------------------------
// Live env policy (the #211 lesson, made distinct for LIVE mode).
// ---------------------------------------------------------------------------

/// Strip the ambient CI repo *identity* so each corpus repo's name resolves to
/// its own directory, but **keep the OIDC token env** so the live scanner can
/// mint its keyless cloud auth for the real LLM call.
///
/// This is the LIVE counterpart to the S2 mock harness's `strip_ci_env`, which
/// also removes `ACTIONS_ID_TOKEN_REQUEST_URL`/`_TOKEN`. We must NOT remove those
/// here — without them the scanner cannot authenticate and every scan dies.
///
/// `GITHUB_REPOSITORY` is the load-bearing one: `get_repository_name`
/// (`src/utils.rs`) prefers it over the scanned path, so leaving it set collapses
/// every corpus repo's identity to `"carrick"` (the runner's repo) and they
/// clobber each other down to a single cache file (#211). `GITHUB_REF` /
/// `GITHUB_EVENT_NAME` are stripped too so `should_upload_data()` is decided by
/// `CARRICK_LOCAL_STORAGE_DIR` (→ true, local cache) rather than the runner's
/// PR/branch context.
fn strip_ci_identity_keep_oidc(cmd: &mut Command) -> &mut Command {
    for var in [
        "GITHUB_REPOSITORY",
        "GITHUB_REF",
        "GITHUB_EVENT_NAME",
        "GITHUB_SHA",
        "GITHUB_RUN_ID",
        "GITHUB_ACTIONS",
        "GITHUB_WORKSPACE",
        "CI",
        // NOTE: ACTIONS_ID_TOKEN_REQUEST_URL / _TOKEN are deliberately KEPT —
        // the live scanner needs them to mint the OIDC token for real LLM auth.
    ] {
        cmd.env_remove(var);
    }
    cmd
}

/// Phase A (live): scan one repo in isolation with the real LLM and persist its
/// `CloudRepoData` to the shared cache. `CARRICK_LOCAL_STORAGE_ISOLATE=1` forces
/// `download_all_repo_data` to return empty (no sibling/cloud leak); the
/// `CARRICK_LOCAL_STORAGE_DIR` presence flips `should_upload_data()` to true so
/// the upload lands in the LOCAL cache, never the real cloud. `CARRICK_OUTPUT_JSON`
/// is deliberately unset (it would skip the upload).
fn phase_a_live(bin: &Path, repo: &Path, cache_dir: &Path) {
    let mut cmd = Command::new(bin);
    cmd.arg(repo)
        .env("CARRICK_LOCAL_STORAGE_DIR", cache_dir)
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env_remove("CARRICK_OUTPUT_JSON")
        // No CARRICK_MOCK_ALL: this is the LIVE path (real LLM).
        .env_remove("CARRICK_MOCK_ALL");
    strip_ci_identity_keep_oidc(&mut cmd);
    let output = cmd.output().unwrap_or_else(|e| {
        panic!(
            "failed to spawn carrick (Phase A live) for {}: {e}",
            repo.display()
        )
    });
    assert!(
        output.status.success(),
        "Phase A live scan of {} exited non-zero:\n{}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Phase A (mock): the offline plumbing-smoke variant. Identical wiring to the
/// live path except `CARRICK_MOCK_ALL=1` substitutes the heuristic mock for the
/// LLM, so it runs without OIDC under plain `cargo test`. Used only by the
/// `#[ignore]`d mock-smoke test.
fn phase_a_mock(bin: &Path, repo: &Path, cache_dir: &Path) {
    let mut cmd = Command::new(bin);
    cmd.arg(repo)
        .env("CARRICK_LOCAL_STORAGE_DIR", cache_dir)
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_MOCK_ALL", "1")
        .env_remove("CARRICK_OUTPUT_JSON");
    // Mock mode needs no cloud auth, so strip OIDC too (matches S2's strip_ci_env).
    strip_ci_identity_keep_oidc(&mut cmd);
    cmd.env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
    let output = cmd.output().unwrap_or_else(|e| {
        panic!(
            "failed to spawn carrick (Phase A mock) for {}: {e}",
            repo.display()
        )
    });
    assert!(
        output.status.success(),
        "Phase A mock scan of {} exited non-zero:\n{}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Assert Phase A persisted *something* for `repo` to the cache. A single-service
/// repo writes `<repo>.json`; a multi-service repo (the monorepo declares
/// `orders-pkg` + `gateway` in its `carrick.json`) writes one
/// `<repo>__<service>.json` per service (LocalDirStorage's multi-service keying).
/// So we assert at least one cache file whose stem is `<repo>` or `<repo>__*`.
fn assert_phase_a_persisted(repo: &Path, cache_dir: &Path) {
    let repo_name = repo
        .file_name()
        .and_then(|s| s.to_str())
        .expect("corpus repo has a name");
    let any = std::fs::read_dir(cache_dir)
        .expect("read cache dir")
        .filter_map(|e| e.ok())
        .any(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("json") {
                return false;
            }
            match p.file_stem().and_then(|s| s.to_str()) {
                Some(stem) => stem == repo_name || stem.starts_with(&format!("{repo_name}__")),
                None => false,
            }
        });
    assert!(
        any,
        "Phase A persisted no cache file for {repo_name} (expected {repo_name}.json or \
         {repo_name}__<service>.json) in {}",
        cache_dir.display()
    );
}

/// Phase B: join the cached repos and emit the merged projection. `ts_check_dir`
/// is auto-discovered by the binary (contract §7 seam), so cross-repo type
/// checking *runs* even though this slice does not score compat. Fails loud if
/// type checking was silently skipped (a missing `ts_check/`), per §7.
fn phase_b(bin: &Path, repo: &Path, cache_dir: &Path, mock: bool) -> EvalProjection {
    let mut cmd = Command::new(bin);
    cmd.arg(repo)
        .env("CARRICK_LOCAL_STORAGE_DIR", cache_dir)
        .env("CARRICK_OUTPUT_JSON", "1")
        .env_remove("CARRICK_LOCAL_STORAGE_ISOLATE");
    if mock {
        cmd.env("CARRICK_MOCK_ALL", "1");
    } else {
        cmd.env_remove("CARRICK_MOCK_ALL");
    }
    strip_ci_identity_keep_oidc(&mut cmd);
    if mock {
        cmd.env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
            .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
    }
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn carrick (Phase B): {e}"));
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "Phase B scan exited non-zero:\n{stderr}"
    );
    // Contract §7 ts_check_dir seam: type checking only runs when the dir is
    // Some. Fail loud if it was silently skipped — even though compat *scoring*
    // is deferred, the seam must stay live so the deferred metric isn't built on
    // silently-absent data.
    assert!(
        !stderr.contains("Skipping type checking"),
        "ts_check/ was not found, so cross-repo type checking was silently skipped. \
         Ensure ts_check/ ships at the repo root.\n{stderr}"
    );
    let stdout = String::from_utf8(output.stdout).expect("Phase B stdout was not UTF-8");
    parse_projection(&stdout).unwrap_or_else(|| {
        panic!("Phase B stdout was not a valid EvalProjection:\n{stdout}");
    })
}

/// Tolerate log noise around the JSON by slicing from the first `{` to the last `}`.
fn parse_projection(stdout: &str) -> Option<EvalProjection> {
    let start = stdout.find('{')?;
    let end = stdout.rfind('}')?;
    serde_json::from_str(stdout.get(start..=end)?).ok()
}

/// Load every repo's `expected.json`, keyed by repo dir name.
fn load_repo_expected(repos: &[PathBuf]) -> Vec<(String, ExpectedRepo)> {
    repos
        .iter()
        .map(|repo| {
            let name = repo
                .file_name()
                .and_then(|s| s.to_str())
                .expect("repo has a name")
                .to_string();
            let text = std::fs::read_to_string(repo.join("expected.json"))
                .unwrap_or_else(|e| panic!("read expected.json for {name}: {e}"));
            let exp: ExpectedRepo = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("parse expected.json for {name}: {e}"));
            (name, exp)
        })
        .collect()
}

fn load_expected_output(corpus: &Path) -> ExpectedOutput {
    let text = std::fs::read_to_string(corpus.join("expected-output.json"))
        .expect("read expected-output.json");
    serde_json::from_str(&text).expect("parse expected-output.json")
}

/// Run the full two-phase loop and return the joined projection. `mock` selects
/// the offline plumbing-smoke path (heuristic LLM, no OIDC) vs the live path.
fn run_two_phase(bin: &Path, corpus: &Path, mock: bool) -> EvalProjection {
    let repos = discover_repos(corpus);
    assert_eq!(
        repos.len(),
        3,
        "expected the 3 top-level corpus repos, found {} in {} — discover_repos must \
         NOT descend into orders-monorepo/packages/*",
        repos.len(),
        corpus.display()
    );
    let cache = tempfile::tempdir().expect("failed to create temp cache dir");
    for repo in &repos {
        if mock {
            phase_a_mock(bin, repo, cache.path());
        } else {
            phase_a_live(bin, repo, cache.path());
        }
        assert_phase_a_persisted(repo, cache.path());
    }
    phase_b(bin, &repos[0], cache.path(), mock)
}

fn emit_record(record: &EvalRunRecord) {
    let line = serde_json::to_string(record).expect("serialise EvalRunRecord");
    let out_dir = manifest_dir().join("target/eval-runs");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("[eval] could not create {}: {e}", out_dir.display());
    } else {
        let path = out_dir.join(format!("xrepo-run-{}.jsonl", record.ts_unix));
        match std::fs::File::create(&path).and_then(|mut f| writeln!(f, "{line}")) {
            Ok(()) => println!("\n[eval] wrote 1 record to {}", path.display()),
            Err(e) => eprintln!("[eval] could not write {}: {e}", path.display()),
        }
    }
    println!("=== EVAL JSONL BEGIN ===");
    println!("{line}");
    println!("=== EVAL JSONL END ===");
}

/// The live cross-repo scorer. Gated behind `#[ignore]` AND `CARRICK_EVAL_LIVE=1`
/// so a plain `cargo test` (or `cargo test --test eval_xrepo`) never triggers a
/// costly real-LLM scan. Run it explicitly:
///
/// ```text
/// CARRICK_EVAL_LIVE=1 cargo test --release --test eval_xrepo -- --ignored \
///     xrepo_live_scorer --test-threads=1 --nocapture
/// ```
#[test]
#[ignore = "live: real LLM scan, costs money — run via eval-xrepo.yml workflow_dispatch"]
fn xrepo_live_scorer() {
    if std::env::var("CARRICK_EVAL_LIVE").map(|v| v.is_empty()) != Ok(false) {
        eprintln!("[eval] CARRICK_EVAL_LIVE not set — skipping live cross-repo scorer.");
        return;
    }
    if std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").is_err() {
        eprintln!(
            "[eval] GitHub Actions OIDC unavailable — skipping live cross-repo scorer \
             (run in CI with `permissions: id-token: write`)."
        );
        return;
    }

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_carrick"));
    let corpus = corpus_dir();
    assert!(corpus.is_dir(), "corpus dir missing: {}", corpus.display());

    let runs_n: usize = std::env::var("CARRICK_EVAL_RUNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v >= 1)
        .unwrap_or(DEFAULT_RUNS);

    let repos = discover_repos(&corpus);
    let repo_expected = load_repo_expected(&repos);
    let expected_output = load_expected_output(&corpus);

    println!("\n=== Cross-repo live scorer ({CORPUS}, N={runs_n}) ===");
    println!("(endpoint-set + cross-repo match P/R/F1 as mean±sd; report-only monitor)\n");

    let mut scores: Vec<RunScore> = Vec::new();
    for run_idx in 1..=runs_n {
        let proj = run_two_phase(&bin, &corpus, false);
        // Extraction diagnostic (report-only, run 1 only): a cross-repo match
        // metric of 0 is otherwise opaque. Surface what the scan produced so a
        // miss is attributable — were the consumer calls extracted (and with
        // what URLs), and did any edges form (with which repo identities)?
        if run_idx == 1 {
            eprintln!(
                "[diag] run 1 projection: {} endpoints, {} calls, {} cross_repo_matches",
                proj.endpoints.len(),
                proj.calls.len(),
                proj.cross_repo_matches.len()
            );
            for c in &proj.calls {
                eprintln!(
                    "[diag]   call {} {}",
                    c.method.clone().unwrap_or_default(),
                    c.path.clone().unwrap_or_default()
                );
            }
            for m in &proj.cross_repo_matches {
                eprintln!(
                    "[diag]   edge {}|{} -> {}|{}",
                    m.producer_repo, m.producer_key, m.consumer_repo, m.consumer_key
                );
            }
        }
        scores.push(score_corpus(&proj, &repo_expected, &expected_output));
        let s = scores.last().unwrap();
        println!(
            "  run {run_idx}/{runs_n}: endpoint F1 {:.2}  match F1 {:.2}",
            s.ep_prf.2, s.match_prf.2
        );
    }
    let n = scores.len();
    assert!(n > 0, "[eval] every run failed");

    let (ep_p, ep_p_sd) = mean_sd(&scores.iter().map(|s| s.ep_prf.0).collect::<Vec<_>>());
    let (ep_r, ep_r_sd) = mean_sd(&scores.iter().map(|s| s.ep_prf.1).collect::<Vec<_>>());
    let (ep_f, ep_f_sd) = mean_sd(&scores.iter().map(|s| s.ep_prf.2).collect::<Vec<_>>());
    let (m_p, m_p_sd) = mean_sd(&scores.iter().map(|s| s.match_prf.0).collect::<Vec<_>>());
    let (m_r, m_r_sd) = mean_sd(&scores.iter().map(|s| s.match_prf.1).collect::<Vec<_>>());
    let (m_f, m_f_sd) = mean_sd(&scores.iter().map(|s| s.match_prf.2).collect::<Vec<_>>());

    println!("\n{CORPUS} (n={n}/{runs_n})");
    println!(
        "  endpoint-set  P {ep_p:.2}±{ep_p_sd:.2}  R {ep_r:.2}±{ep_r_sd:.2}  F1 {ep_f:.2}±{ep_f_sd:.2}"
    );
    println!(
        "  xrepo match   P {m_p:.2}±{m_p_sd:.2}  R {m_r:.2}±{m_r_sd:.2}  F1 {m_f:.2}±{m_f_sd:.2}"
    );
    println!("=== end cross-repo live scorer (report-only; no assertions on scores) ===\n");

    let record = EvalRunRecord {
        schema_version: RECORD_SCHEMA_VERSION,
        ts_unix: now_unix(),
        scanner_version: env!("CARGO_PKG_VERSION").to_string(),
        carrick_sha: std::env::var("GITHUB_SHA").unwrap_or_else(|_| "local".to_string()),
        github_run_id: std::env::var("GITHUB_RUN_ID").ok(),
        fixture: CORPUS.to_string(),
        runs_requested: runs_n,
        runs_effective: n,
        model_id: None,
        prompt_hash: None,
        ep_precision_mean: ep_p,
        ep_precision_sd: ep_p_sd,
        ep_recall_mean: ep_r,
        ep_recall_sd: ep_r_sd,
        ep_f1_mean: ep_f,
        ep_f1_sd: ep_f_sd,
        ep_pass_at_k: 0.0,
        ep_pass_pow_k: 0.0,
        // Calls not scored by this slice — emit a perfect-empty (0 found, 0 expected).
        call_precision_mean: 1.0,
        call_precision_sd: 0.0,
        call_recall_mean: 1.0,
        call_recall_sd: 0.0,
        call_f1_mean: 1.0,
        call_f1_sd: 0.0,
        call_pass_at_k: 1.0,
        call_pass_pow_k: 1.0,
        note: Some("S4 thin slice: endpoint-set + cross-repo match only".to_string()),
        tier: Some("xrepo".to_string()),
        corpus: Some(CORPUS.to_string()),
        match_precision_mean: Some(m_p),
        match_precision_sd: Some(m_p_sd),
        match_recall_mean: Some(m_r),
        match_recall_sd: Some(m_r_sd),
        match_f1_mean: Some(m_f),
        match_f1_sd: Some(m_f_sd),
        // DEFERRED facets (contract §6 rows 4-6, 8-10): left None.
        type_anchor_accuracy_mean: None,
        type_anchor_accuracy_sd: None,
        type_resolution_accuracy_mean: None,
        type_resolution_accuracy_sd: None,
        compat_verdict_accuracy_mean: None,
        compat_verdict_accuracy_sd: None,
        owner_accuracy_mean: None,
        owner_accuracy_sd: None,
        decoy_leak_mean: None,
        decoy_leak_sd: None,
        dep_f1_mean: None,
        dep_f1_sd: None,
    };
    emit_record(&record);
}

/// MOCK-mode plumbing smoke check (offline, no OIDC, runs under plain
/// `cargo test --test eval_xrepo -- --ignored mock_smoke`). Proves the two-phase
/// harness spans all 3 repos and produces a parseable `EvalProjection` without
/// crashing. Matches may be ~empty under the heuristic mock — that is EXPECTED;
/// this asserts plumbing, not accuracy. `#[ignore]` because it spawns the binary
/// over the corpus (slow) and is not part of the CI Test Suite allowlist.
#[test]
#[ignore = "mock plumbing smoke: spawns the binary over the corpus (slow, offline)"]
fn xrepo_mock_plumbing_smoke() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_carrick"));
    let corpus = corpus_dir();
    assert!(corpus.is_dir(), "corpus dir missing: {}", corpus.display());

    let proj = run_two_phase(&bin, &corpus, true);
    // Plumbing assertion only: the join produced a non-empty projection spanning
    // the corpus. Accuracy (endpoint/match F1) is NOT asserted in mock mode.
    assert!(
        !proj.endpoints.is_empty(),
        "mock two-phase join produced no endpoints across the 3 repos — plumbing broken"
    );
    println!(
        "[eval] mock smoke: {} endpoints, {} cross-repo matches across the joined corpus",
        proj.endpoints.len(),
        proj.cross_repo_matches.len()
    );
}

// ---------------------------------------------------------------------------
// Scoring-math unit tests — these DO run under plain `cargo test` (no #[ignore],
// no subprocess, no LLM). Synthetic projections + the real corpus labels →
// known numbers.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod scoring_tests {
    use super::*;

    fn ep(method: &str, path: &str) -> EvalOp {
        EvalOp {
            protocol: "http".to_string(),
            method: Some(method.to_string()),
            path: Some(path.to_string()),
        }
    }

    fn cm(p_repo: &str, p_key: &str, c_repo: &str, c_key: &str) -> EvalCrossRepoMatch {
        EvalCrossRepoMatch {
            producer_repo: p_repo.to_string(),
            producer_key: p_key.to_string(),
            consumer_repo: c_repo.to_string(),
            consumer_key: c_key.to_string(),
        }
    }

    #[test]
    fn norm_path_collapses_all_three_param_syntaxes() {
        assert_eq!(norm_path("/orders/:id"), "/orders/:param");
        assert_eq!(norm_path("/orders/{id}"), "/orders/:param");
        assert_eq!(norm_path("/orders/[id]"), "/orders/:param");
        // Trailing slash stripped (len > 1); root preserved.
        assert_eq!(norm_path("/orders/"), "/orders");
        assert_eq!(norm_path("/"), "/");
    }

    #[test]
    fn norm_key_uppercases_method_and_normalizes_path() {
        assert_eq!(norm_key("http|get|/orders/{id}"), "http|GET|/orders/:param");
        assert_eq!(norm_key("http|GET|/orders/:id"), "http|GET|/orders/:param");
        // A malformed key (no pipes) passes through unchanged.
        assert_eq!(norm_key("not-a-key"), "not-a-key");
    }

    #[test]
    fn prf_contract_conventions() {
        // Nothing expected, nothing found → perfect (the roadmap-zero convention).
        assert_eq!(prf(0, 0, 0), (1.0, 1.0, 1.0));
        // Something found into an empty expectation → precision 0.
        assert_eq!(prf(0, 2, 0).0, 0.0);
        // Perfect set match.
        assert_eq!(prf(3, 3, 3), (1.0, 1.0, 1.0));
        // Half recall, full precision.
        let (p, r, _) = prf(1, 1, 2);
        assert_eq!((p, r), (1.0, 0.5));
    }

    #[test]
    fn mean_sd_sample_variance() {
        let (m, sd) = mean_sd(&[1.0, 1.0, 1.0]);
        assert_eq!(m, 1.0);
        assert_eq!(sd, 0.0);
        let (m, sd) = mean_sd(&[0.0, 1.0]);
        assert_eq!(m, 0.5);
        // sample sd of {0,1} = sqrt(0.5) ≈ 0.707
        assert!((sd - 0.5_f64.sqrt()).abs() < 1e-9);
        // n<1 guard.
        assert_eq!(mean_sd(&[]), (0.0, 0.0));
    }

    #[test]
    fn endpoint_set_perfect_and_partial() {
        let expected = vec![
            ("GET".to_string(), "/orders/:param".to_string()),
            ("POST".to_string(), "/payments".to_string()),
        ];
        // Perfect: same set, different param syntax normalizes to a match.
        let found: HashSet<_> = [ep("GET", "/orders/{id}"), ep("post", "/payments")]
            .iter()
            .filter_map(|o| {
                Some((
                    o.method.clone()?.to_uppercase(),
                    norm_path(o.path.as_deref()?),
                ))
            })
            .collect();
        assert_eq!(score_endpoint_set(&expected, &found), (1.0, 1.0, 1.0));

        // One extra spurious endpoint → precision drops, recall stays 1.
        let found: HashSet<_> = [
            ep("GET", "/orders/9"),
            ep("POST", "/payments"),
            ep("DELETE", "/spurious"),
        ]
        .iter()
        .filter_map(|o| {
            Some((
                o.method.clone()?.to_uppercase(),
                norm_path(o.path.as_deref()?),
            ))
        })
        .collect();
        let (p, r, _) = score_endpoint_set(&expected, &found);
        // /orders/9 has no param segment, so it does NOT match /orders/:param.
        // Matched: only POST /payments. found=3, expected=2, tp=1.
        assert!((p - 1.0 / 3.0).abs() < 1e-9);
        assert!((r - 0.5).abs() < 1e-9);
    }

    #[test]
    fn match_set_exact_with_normalization() {
        let expected = vec![
            ExpMatch {
                producer_repo: "orders-monorepo".to_string(),
                producer_key: "http|GET|/orders/:param".to_string(),
                consumer_repo: "payments-svc".to_string(),
                consumer_key: "http|GET|/orders/:param".to_string(),
            },
            ExpMatch {
                producer_repo: "payments-svc".to_string(),
                producer_key: "http|POST|/payments".to_string(),
                consumer_repo: "web-frontend".to_string(),
                consumer_key: "http|POST|/payments".to_string(),
            },
        ];
        // Found edges use a different param syntax ({id}) + lowercased method;
        // normalization makes the first edge match exactly. Missing the second.
        let found = vec![cm(
            "orders-monorepo",
            "http|get|/orders/{id}",
            "payments-svc",
            "http|GET|/orders/[id]",
        )];
        let (p, r, _f) = score_matches(&expected, &found);
        assert_eq!(p, 1.0, "the one found edge matched after normalization");
        assert_eq!(r, 0.5, "one of two expected edges found");
    }

    /// Drive the scorer against the REAL corpus labels with a synthetic
    /// projection that exactly reproduces the answer key → both metrics 1.0.
    /// Pins the contract: corpus labels + a perfect scan = perfect score.
    #[test]
    fn perfect_synthetic_projection_against_real_corpus() {
        let corpus = corpus_dir();
        let repos = discover_repos(&corpus);
        assert_eq!(repos.len(), 3, "the 3 top-level corpus repos");
        let repo_expected = load_repo_expected(&repos);
        let expected_output = load_expected_output(&corpus);

        // Build a projection whose endpoint set equals the union of all repos'
        // expected endpoints, and whose matches equal the expected edges.
        let endpoints: Vec<EvalOp> = corpus_expected_endpoints(&repo_expected)
            .into_iter()
            .map(|(m, p)| ep(&m, &p))
            .collect();
        let cross_repo_matches: Vec<EvalCrossRepoMatch> = expected_output
            .matches
            .iter()
            .map(|m| {
                cm(
                    &m.producer_repo,
                    &m.producer_key,
                    &m.consumer_repo,
                    &m.consumer_key,
                )
            })
            .collect();
        let proj = EvalProjection {
            endpoints,
            calls: vec![],
            cross_repo_matches,
        };

        let score = score_corpus(&proj, &repo_expected, &expected_output);
        assert_eq!(score.ep_prf, (1.0, 1.0, 1.0), "perfect endpoint set");
        assert_eq!(score.match_prf, (1.0, 1.0, 1.0), "perfect match set");
    }

    /// An empty projection against the real corpus → recall 0 on both metrics
    /// (precision is the §1.4 found==0 convention: 0.0 since something IS
    /// expected). Pins the worst case so a silently-dead scan reads as a zero,
    /// not a spurious pass.
    #[test]
    fn empty_projection_against_real_corpus_scores_zero_recall() {
        let corpus = corpus_dir();
        let repos = discover_repos(&corpus);
        let repo_expected = load_repo_expected(&repos);
        let expected_output = load_expected_output(&corpus);

        let proj = EvalProjection {
            endpoints: vec![],
            calls: vec![],
            cross_repo_matches: vec![],
        };
        let score = score_corpus(&proj, &repo_expected, &expected_output);
        assert_eq!(score.ep_prf.1, 0.0, "no endpoints found → recall 0");
        assert_eq!(
            score.ep_prf.0, 0.0,
            "found==0 with expected>0 → precision 0"
        );
        assert_eq!(score.match_prf.1, 0.0, "no matches found → recall 0");
    }

    #[test]
    fn eval_run_record_round_trips() {
        let rec = EvalRunRecord {
            schema_version: RECORD_SCHEMA_VERSION,
            ts_unix: 1_700_000_000,
            scanner_version: "0.1.40".into(),
            carrick_sha: "local".into(),
            github_run_id: None,
            fixture: CORPUS.into(),
            runs_requested: 5,
            runs_effective: 5,
            model_id: None,
            prompt_hash: None,
            ep_precision_mean: 0.8,
            ep_precision_sd: 0.1,
            ep_recall_mean: 0.7,
            ep_recall_sd: 0.12,
            ep_f1_mean: 0.74,
            ep_f1_sd: 0.11,
            ep_pass_at_k: 0.0,
            ep_pass_pow_k: 0.0,
            call_precision_mean: 1.0,
            call_precision_sd: 0.0,
            call_recall_mean: 1.0,
            call_recall_sd: 0.0,
            call_f1_mean: 1.0,
            call_f1_sd: 0.0,
            call_pass_at_k: 1.0,
            call_pass_pow_k: 1.0,
            note: Some("S4 thin slice".into()),
            tier: Some("xrepo".into()),
            corpus: Some(CORPUS.into()),
            match_precision_mean: Some(0.6),
            match_precision_sd: Some(0.2),
            match_recall_mean: Some(0.5),
            match_recall_sd: Some(0.15),
            match_f1_mean: Some(0.55),
            match_f1_sd: Some(0.17),
            type_anchor_accuracy_mean: None,
            type_anchor_accuracy_sd: None,
            type_resolution_accuracy_mean: None,
            type_resolution_accuracy_sd: None,
            compat_verdict_accuracy_mean: None,
            compat_verdict_accuracy_sd: None,
            owner_accuracy_mean: None,
            owner_accuracy_sd: None,
            decoy_leak_mean: None,
            decoy_leak_sd: None,
            dep_f1_mean: None,
            dep_f1_sd: None,
        };
        let line = serde_json::to_string(&rec).unwrap();
        let back: EvalRunRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(back.tier.as_deref(), Some("xrepo"));
        assert_eq!(back.corpus.as_deref(), Some(CORPUS));
        assert_eq!(back.match_f1_mean, Some(0.55));
        // Deferred facets stay None (and are omitted from JSON).
        assert!(back.compat_verdict_accuracy_mean.is_none());
        assert!(!line.contains("compat_verdict_accuracy_mean"));
        assert!(!line.contains("dep_f1_mean"));
    }
}
