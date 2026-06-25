//! Offline two-phase cross-repo eval harness (eval slice S2, #201).
//!
//! Drives the *real* scanner binary over a multi-repo corpus entirely offline,
//! the same way the cross-repo accuracy eval will, and asserts the joined
//! [`EvalProjection`] reflects every corpus repo. It exercises the
//! `LocalDirStorage` backend introduced in this slice:
//!
//! - **Phase A (per repo, isolated):** each corpus repo is scanned in its own
//!   subprocess with `CARRICK_LOCAL_STORAGE_ISOLATE=1`, so the engine's
//!   `download_all_repo_data` returns *empty* — no real-cloud sibling data and
//!   no other corpus repo can leak into the per-repo scan (Tier-A fidelity).
//!   The scan uploads that repo's `CloudRepoData` to a shared cache dir.
//! - **Phase B (join):** one more subprocess runs without the isolate flag, so
//!   `download_all_repo_data` reads back *all* the cached repos. The engine's
//!   `build_cross_repo_analyzer` joins them and, with `CARRICK_OUTPUT_JSON` set,
//!   emits the machine-readable projection of the merged result.
//!
//! The LLM is mocked from each repo's committed `__llm__/` cassette
//! (`CARRICK_MOCK_FIXTURE_DIR`), so the run is deterministic, network-free and
//! OIDC-free — it runs under plain `cargo test`.
//!
//! `ts_check_dir` is auto-discovered by the binary (it resolves
//! `<CARGO_MANIFEST_DIR>/ts_check`), so Phase B supplies it and cross-repo type
//! checking *runs* rather than being silently skipped. The corpus fixtures here
//! carry no resolvable `.d.ts`, so the check itself only `warn!`s — but the
//! `ts_check_dir` path is wired, which is the load-bearing seam the contract's
//! §7 guard depends on (see the doc-comment on `phase_b`).
//!
//! INTEGRATION SEAM (S1, #200): today's `EvalProjection` carries only
//! `endpoints` + `calls`, so Phase B asserts the merged set spans all repos.
//! Once S1 lands `cross_repo_matches` on the projection, this is where the
//! producer→consumer match edges (e.g. repo-b/repo-c → repo-a `/api/users`)
//! will be asserted directly — see `assert_joined_projection`.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Mirror of `carrick::eval_output::EvalProjection` for deserializing the
/// binary's `CARRICK_OUTPUT_JSON` stdout. Kept local (rather than depending on
/// the lib type) so the harness reads the projection purely as a wire contract.
#[derive(Debug, Deserialize)]
struct EvalProjection {
    endpoints: Vec<EvalOp>,
    calls: Vec<EvalOp>,
}

#[derive(Debug, Deserialize)]
struct EvalOp {
    key: String,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The corpus directory to run the harness over. Overridable via
/// `CARRICK_XREPO_CORPUS` (absolute, or relative to the repo root); defaults to
/// the committed `scenario-3-cross-repo-success` constellation.
fn corpus_dir() -> PathBuf {
    match std::env::var("CARRICK_XREPO_CORPUS") {
        Ok(p) => {
            let path = PathBuf::from(&p);
            if path.is_absolute() {
                path
            } else {
                repo_root().join(path)
            }
        }
        Err(_) => repo_root().join("tests/fixtures/scenario-3-cross-repo-success"),
    }
}

/// The corpus repos: every immediate subdirectory of the corpus dir that holds
/// a `package.json`. Sorted for deterministic Phase-A ordering.
fn discover_repos(corpus: &Path) -> Vec<PathBuf> {
    let mut repos: Vec<PathBuf> = std::fs::read_dir(corpus)
        .unwrap_or_else(|e| panic!("failed to read corpus dir {}: {e}", corpus.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join("package.json").exists())
        .collect();
    repos.sort();
    repos
}

/// The per-repo LLM cassette dir (`<repo>/__llm__/`). The harness requires each
/// corpus repo to ship one so the run is deterministic.
fn cassette_dir(repo: &Path) -> PathBuf {
    repo.join("__llm__")
}

/// Strip the ambient CI / GitHub-Actions context so the eval subprocess runs
/// deterministically wherever the harness is invoked. In CI these are set on
/// the runner and would otherwise leak in: `GITHUB_REPOSITORY` makes repo
/// identity resolve to the outer repo ("carrick") instead of the scanned corpus
/// dir — clobbering every cached repo down to one file — and the PR/branch vars
/// flip `should_upload_data()` off.
fn strip_ci_env(cmd: &mut Command) -> &mut Command {
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
    ] {
        cmd.env_remove(var);
    }
    cmd
}

/// Phase A: scan one repo in isolation and persist its `CloudRepoData` to the
/// shared cache dir. `CARRICK_LOCAL_STORAGE_ISOLATE=1` forces
/// `download_all_repo_data` to return empty, so neither the real cloud nor a
/// sibling corpus repo can contaminate this per-repo scan. `CARRICK_OUTPUT_JSON`
/// is deliberately *unset* here: it would flip `should_upload_data()` to false
/// and the upload (the whole point of Phase A) would be skipped.
fn phase_a(bin: &Path, repo: &Path, cache_dir: &Path) {
    let cassettes = cassette_dir(repo);
    assert!(
        cassettes.exists(),
        "corpus repo {} is missing its __llm__/ cassette dir (required for a \
         deterministic offline run)",
        repo.display()
    );

    let mut cmd = Command::new(bin);
    cmd.arg(repo)
        .env("CARRICK_LOCAL_STORAGE_DIR", cache_dir)
        .env("CARRICK_LOCAL_STORAGE_ISOLATE", "1")
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", cassettes.display()),
        )
        // CARRICK_OUTPUT_JSON unset so should_upload_data() keeps the upload on;
        // strip_ci_env (below) ties repo identity to the scanned dir, not the
        // runner's GITHUB_REPOSITORY (which would name every corpus repo "carrick").
        .env_remove("CARRICK_OUTPUT_JSON");
    strip_ci_env(&mut cmd);
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn carrick for {}: {e}", repo.display()));

    assert!(
        output.status.success(),
        "Phase A scan of {} exited non-zero:\n{}",
        repo.display(),
        String::from_utf8_lossy(&output.stderr)
    );

    let repo_name = repo
        .file_name()
        .and_then(|s| s.to_str())
        .expect("corpus repo has a name");
    let cached = cache_dir.join(format!("{repo_name}.json"));
    assert!(
        cached.exists(),
        "Phase A did not persist {} to the cache dir ({})",
        repo_name,
        cached.display()
    );
}

/// Phase B: scan one (arbitrary) corpus repo without the isolate flag, so
/// `download_all_repo_data` reads back every cached repo and
/// `build_cross_repo_analyzer` joins them. `CARRICK_OUTPUT_JSON` makes the
/// engine emit the merged `EvalProjection` to stdout.
///
/// `ts_check_dir` is auto-discovered by the binary, so cross-repo type checking
/// *runs* (the contract's §7 trap is that a missing dir silently absents compat
/// data). The harness asserts type checking was not silently skipped.
fn phase_b(bin: &Path, repo: &Path, cache_dir: &Path) -> (EvalProjection, String) {
    let cassettes = cassette_dir(repo);
    let mut cmd = Command::new(bin);
    cmd.arg(repo)
        .env("CARRICK_LOCAL_STORAGE_DIR", cache_dir)
        // No CARRICK_LOCAL_STORAGE_ISOLATE: download returns all cached repos.
        .env("CARRICK_MOCK_ALL", "1")
        .env(
            "CARRICK_MOCK_FIXTURE_DIR",
            format!("{}/", cassettes.display()),
        )
        .env("CARRICK_OUTPUT_JSON", "1")
        .env_remove("CARRICK_LOCAL_STORAGE_ISOLATE");
    strip_ci_env(&mut cmd);
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn carrick (Phase B): {e}"));

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "Phase B scan exited non-zero:\n{stderr}"
    );

    // The §7 ts_check_dir guard: cross-repo type checking only runs when
    // ts_check_dir is Some. If the binary couldn't find ts_check/ it logs a
    // "Skipping type checking" warning — fail loud rather than let a future
    // compat scorer misread silently-absent verdicts as "all compatible".
    assert!(
        !stderr.contains("Skipping type checking"),
        "ts_check/ was not found, so cross-repo type checking was silently \
         skipped — compat data would be absent, not 'all compatible'. \
         discover_ts_check_path() resolves ts_check/run-type-checking.ts; \
         ensure the ts_check/ dir is present at the repo root (it ships with \
         the checkout).\n{stderr}"
    );

    let stdout = String::from_utf8(output.stdout).expect("Phase B stdout was not UTF-8");
    let projection: EvalProjection =
        serde_json::from_str(&stdout).expect("Phase B stdout was not a valid EvalProjection");
    (projection, stderr)
}

/// Assert the merged projection reflects the whole corpus. For scenario-3 the
/// producer endpoints from repo-a/repo-b and the consumer calls from
/// repo-b/repo-c must all appear in the single joined projection — proof that
/// Phase B downloaded all N cached repos and joined them.
///
/// INTEGRATION SEAM (S1, #200): once `cross_repo_matches` lands on
/// `EvalProjection`, add the edge assertions here (e.g. an edge from the
/// repo-b/repo-c `/api/users` calls to the repo-a producer).
fn assert_joined_projection(projection: &EvalProjection, corpus: &Path) {
    assert!(
        !projection.endpoints.is_empty() || !projection.calls.is_empty(),
        "joined projection is empty — no endpoints or calls survived the two-phase loop"
    );

    let is_scenario_3 = corpus.ends_with("scenario-3-cross-repo-success");
    if !is_scenario_3 {
        // For an arbitrary corpus we can only assert non-emptiness; the
        // scenario-3 expectations below are specific to that fixture.
        return;
    }

    let endpoint_keys: Vec<&str> = projection
        .endpoints
        .iter()
        .map(|e| e.key.as_str())
        .collect();
    let call_keys: Vec<&str> = projection.calls.iter().map(|c| c.key.as_str()).collect();

    // Producers from two different repos both appear in the merged projection.
    assert!(
        endpoint_keys.contains(&"http|GET|/api/users"),
        "missing repo-a producer GET /api/users; got endpoints {endpoint_keys:?}"
    );
    assert!(
        endpoint_keys.contains(&"http|GET|/api/products"),
        "missing repo-b producer GET /api/products; got endpoints {endpoint_keys:?}"
    );

    // Consumer calls (from repo-b and repo-c) carry their real targets — proof
    // the cassettes replayed real URLs (not the heuristic placeholder) and that
    // the joined download surfaced cross-repo consumers.
    assert!(
        call_keys.iter().any(|k| k.contains("/api/users")),
        "missing consumer call to /api/users; got calls {call_keys:?}"
    );
    assert!(
        call_keys.iter().any(|k| k.contains("/api/products")),
        "missing consumer call to /api/products; got calls {call_keys:?}"
    );
}

#[test]
fn xrepo_two_phase_harness_joins_corpus() {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_carrick"));
    let corpus = corpus_dir();
    assert!(
        corpus.is_dir(),
        "corpus dir does not exist: {}",
        corpus.display()
    );

    let repos = discover_repos(&corpus);
    assert!(
        repos.len() >= 2,
        "cross-repo harness needs >=2 corpus repos, found {} in {}",
        repos.len(),
        corpus.display()
    );

    // Each phase runs the REAL binary in its own subprocess (max fidelity); the
    // in-process engine route is the clean fallback only. A fresh cache dir per
    // run keeps phases hermetic.
    let cache = tempfile::tempdir().expect("failed to create temp cache dir");

    // Phase A: scan every repo in isolation, persisting each to the cache.
    for repo in &repos {
        phase_a(&bin, repo, cache.path());
    }

    // Phase B: join the cached repos and emit the merged projection. Scanning
    // the first repo is arbitrary — the projection reflects the joined set.
    let (projection, _stderr) = phase_b(&bin, &repos[0], cache.path());

    assert_joined_projection(&projection, &corpus);
}
