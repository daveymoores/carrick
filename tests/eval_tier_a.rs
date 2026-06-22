//! Tier-A extraction-quality scorer (evals plan, Slice 2).
//!
//! Runs the release scanner against each framework fixture **N times** (default 5,
//! override with `CARRICK_EVAL_RUNS`) in `CARRICK_OUTPUT_JSON` mode, scores each
//! run against the fixture's `expected.json`, and reports:
//!   - endpoint & call precision/recall/F1 as **mean ± stddev** across runs, and
//!   - **pass@k / pass^k** per ground-truth element — found in *at least one* run
//!     (capability ceiling) vs found in *all* runs (consistency). The pass@k −
//!     pass^k gap is the run-to-run wobble, made explicit.
//!
//! **Report-only:** asserts only a non-flaky floor (≥1 run parsed + endpoint
//! pass@k > 0), because the live LLM is nondeterministic and a strict assertion
//! would flake. A transient failed run is skipped (effective n is reported), not
//! fatal unless *every* run for a fixture fails.
//!
//! Auth is GitHub Actions OIDC — the scanner is keyless (no API-key path). The
//! runner exposes `ACTIONS_ID_TOKEN_REQUEST_URL`/`_TOKEN` only with
//! `permissions: id-token: write`; locally (no OIDC) this test skips cleanly.
//! Runner: `.github/workflows/eval-tier-a.yml`.
//!
//! The API endpoint is compile-time (`build.rs`), so build with it set:
//!   CARRICK_API_ENDPOINT="https://api.carrick.tools" cargo build --release
//!   cargo test --release --test eval_tier_a -- --test-threads=1 --nocapture

use carrick::eval_output::EvalProjection;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURES: &[&str] = &["koa-api", "fastify-api", "hapi-api", "nestjs-api"];
const DEFAULT_RUNS: usize = 5;

#[derive(Debug, Deserialize)]
struct Expected {
    #[serde(default)]
    endpoints: Vec<ExpEndpoint>,
    #[serde(default)]
    calls: Vec<ExpCall>,
}

#[derive(Debug, Deserialize)]
struct ExpEndpoint {
    method: String,
    path: String,
}

#[derive(Debug, Deserialize)]
struct ExpCall {
    method: String,
    host_contains: String,
    path_contains: String,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Collapse `:x` / `{x}` / `[x]` param syntaxes to `:param` and strip a trailing
/// slash, so the Hapi `{id}` vs Express-style `:id` split (issue #167) doesn't
/// cause false mismatches.
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

/// Mean and sample stddev (n-1). stddev is 0.0 for n < 2.
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

/// Run the scanner in JSON mode, retrying once on empty stdout (a known transient
/// when the cloud/LLM hiccups). The OIDC env (`ACTIONS_ID_TOKEN_REQUEST_URL`/
/// `_TOKEN`) is inherited from the runner; the API endpoint is compile-time.
fn run_scanner(bin: &Path, fixture_dir: &Path) -> Option<String> {
    for attempt in 1..=2 {
        let output = Command::new(bin)
            .arg(fixture_dir)
            .env("CARRICK_OUTPUT_JSON", "1")
            .output()
            .expect("failed to spawn the carrick binary");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if !stdout.trim().is_empty() {
            return Some(stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<&str> = stderr.lines().collect();
        let tail = lines[lines.len().saturating_sub(15)..].join("\n");
        eprintln!(
            "[eval] empty stdout for {} (attempt {}/2): {}\n--- stderr (last 15 lines) ---\n{}",
            fixture_dir.display(),
            attempt,
            output.status,
            tail
        );
    }
    None
}

/// Tolerate log noise around the JSON by slicing from the first `{` to the last `}`.
fn parse_projection(stdout: &str) -> Option<EvalProjection> {
    let start = stdout.find('{')?;
    let end = stdout.rfind('}')?;
    serde_json::from_str(stdout.get(start..=end)?).ok()
}

/// One run's scores plus per-ground-truth-element hit flags (for pass@k/pass^k).
struct RunScore {
    ep_prf: (f64, f64, f64),
    call_prf: (f64, f64, f64),
    ep_hits: Vec<bool>,
    call_hits: Vec<bool>,
}

fn score_run(
    proj: &EvalProjection,
    expected_eps: &[(String, String)],
    expected: &Expected,
) -> RunScore {
    // Endpoints: set comparison over (METHOD, normalised path), HTTP only.
    let expected_set: HashSet<(String, String)> = expected_eps.iter().cloned().collect();
    let found_eps: HashSet<(String, String)> = proj
        .endpoints
        .iter()
        .filter(|o| o.protocol == "http")
        .filter_map(|o| {
            Some((
                o.method.clone()?.to_uppercase(),
                norm_path(o.path.as_deref()?),
            ))
        })
        .collect();
    let ep_tp = expected_set.intersection(&found_eps).count();
    let ep_prf = prf(ep_tp, found_eps.len(), expected_set.len());
    let ep_hits: Vec<bool> = expected_eps.iter().map(|e| found_eps.contains(e)).collect();

    // Calls: fuzzy match on method + host_contains + path_contains, HTTP only.
    let found_calls: Vec<(String, String)> = proj
        .calls
        .iter()
        .filter(|o| o.protocol == "http")
        .filter_map(|o| {
            Some((
                o.method.clone().unwrap_or_default().to_uppercase(),
                o.path.clone()?,
            ))
        })
        .collect();
    let matches = |e: &ExpCall, c: &(String, String)| {
        c.0 == e.method.to_uppercase()
            && c.1.contains(&e.host_contains)
            && c.1.contains(&e.path_contains)
    };
    let call_hits: Vec<bool> = expected
        .calls
        .iter()
        .map(|e| found_calls.iter().any(|c| matches(e, c)))
        .collect();
    let matched_expected = call_hits.iter().filter(|h| **h).count();
    let matched_found = found_calls
        .iter()
        .filter(|c| expected.calls.iter().any(|e| matches(e, c)))
        .count();
    let call_p = if found_calls.is_empty() {
        if expected.calls.is_empty() { 1.0 } else { 0.0 }
    } else {
        matched_found as f64 / found_calls.len() as f64
    };
    let call_r = if expected.calls.is_empty() {
        1.0
    } else {
        matched_expected as f64 / expected.calls.len() as f64
    };
    let call_f1 = if call_p + call_r == 0.0 {
        0.0
    } else {
        2.0 * call_p * call_r / (call_p + call_r)
    };

    RunScore {
        ep_prf,
        call_prf: (call_p, call_r, call_f1),
        ep_hits,
        call_hits,
    }
}

/// pass@k (any run) and pass^k (all runs) over a set of per-element hit columns.
/// Returns (pass_at_k, pass_pow_k); both 1.0 when there are no elements.
fn pass_rates(hits_per_run: &[Vec<bool>], num_elems: usize) -> (f64, f64) {
    if num_elems == 0 {
        return (1.0, 1.0);
    }
    let n = hits_per_run.len();
    let mut any = 0usize;
    let mut all = 0usize;
    for i in 0..num_elems {
        let hits = hits_per_run.iter().filter(|run| run[i]).count();
        if hits >= 1 {
            any += 1;
        }
        if hits == n {
            all += 1;
        }
    }
    (any as f64 / num_elems as f64, all as f64 / num_elems as f64)
}

#[test]
fn tier_a_extraction_quality() {
    // The scanner authenticates only via GitHub Actions OIDC. Without a runner
    // granting `id-token: write`, no real scan can run — skip cleanly.
    if std::env::var("ACTIONS_ID_TOKEN_REQUEST_URL").is_err() {
        eprintln!(
            "[eval] GitHub Actions OIDC unavailable — skipping Tier-A scorer \
             (run in CI with `permissions: id-token: write`)."
        );
        return;
    }

    let bin = manifest_dir().join("target/release/carrick");
    if !bin.exists() {
        eprintln!(
            "[eval] release binary missing at {} — build first: \
             CARRICK_API_ENDPOINT=\"https://api.carrick.tools\" cargo build --release",
            bin.display()
        );
        return;
    }

    let runs_n: usize = std::env::var("CARRICK_EVAL_RUNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v >= 1)
        .unwrap_or(DEFAULT_RUNS);

    println!("\n=== Tier-A extraction quality (evals Slice 2, N={runs_n}) ===");
    println!("(P/R/F1 as mean±sd across runs; pass@k = found in any run, pass^k = found in all)\n");

    // Accumulators for the cross-fixture MEAN row.
    let nf = FIXTURES.len() as f64;
    let (mut sum_ep_f1, mut sum_call_f1) = (0.0, 0.0);
    let (mut sum_ep_passpow, mut sum_call_passpow) = (0.0, 0.0);

    for name in FIXTURES {
        let fixture_dir = manifest_dir().join("tests/fixtures").join(name);
        let expected: Expected = serde_json::from_str(
            &std::fs::read_to_string(fixture_dir.join("expected.json"))
                .unwrap_or_else(|e| panic!("read expected.json for {name}: {e}")),
        )
        .unwrap_or_else(|e| panic!("parse expected.json for {name}: {e}"));

        let expected_eps: Vec<(String, String)> = expected
            .endpoints
            .iter()
            .map(|e| (e.method.to_uppercase(), norm_path(&e.path)))
            .collect();

        // N runs; skip a transient failure, keep the rest.
        let mut scores: Vec<RunScore> = Vec::new();
        for run_idx in 1..=runs_n {
            match run_scanner(&bin, &fixture_dir)
                .as_deref()
                .and_then(parse_projection)
            {
                Some(proj) => scores.push(score_run(&proj, &expected_eps, &expected)),
                None => eprintln!(
                    "[eval] {name}: run {run_idx}/{runs_n} produced no parseable output — skipped"
                ),
            }
        }
        let n = scores.len();
        assert!(n > 0, "[eval] {name}: all {runs_n} runs failed (cloud/LLM)");

        let (ep_p, ep_p_sd) = mean_sd(&scores.iter().map(|s| s.ep_prf.0).collect::<Vec<_>>());
        let (ep_r, ep_r_sd) = mean_sd(&scores.iter().map(|s| s.ep_prf.1).collect::<Vec<_>>());
        let (ep_f, ep_f_sd) = mean_sd(&scores.iter().map(|s| s.ep_prf.2).collect::<Vec<_>>());
        let (cl_p, cl_p_sd) = mean_sd(&scores.iter().map(|s| s.call_prf.0).collect::<Vec<_>>());
        let (cl_r, cl_r_sd) = mean_sd(&scores.iter().map(|s| s.call_prf.1).collect::<Vec<_>>());
        let (cl_f, cl_f_sd) = mean_sd(&scores.iter().map(|s| s.call_prf.2).collect::<Vec<_>>());

        let ep_hits: Vec<Vec<bool>> = scores.iter().map(|s| s.ep_hits.clone()).collect();
        let call_hits: Vec<Vec<bool>> = scores.iter().map(|s| s.call_hits.clone()).collect();
        let (ep_at, ep_pow) = pass_rates(&ep_hits, expected_eps.len());
        let (cl_at, cl_pow) = pass_rates(&call_hits, expected.calls.len());

        let neff = if n < runs_n {
            format!(" (n={n}/{runs_n})")
        } else {
            String::new()
        };
        println!("{name}{neff}");
        println!(
            "  endpoints  P {ep_p:.2}±{ep_p_sd:.2}  R {ep_r:.2}±{ep_r_sd:.2}  F1 {ep_f:.2}±{ep_f_sd:.2}   pass@{n} {ep_at:.2}  pass^{n} {ep_pow:.2}"
        );
        println!(
            "  calls      P {cl_p:.2}±{cl_p_sd:.2}  R {cl_r:.2}±{cl_r_sd:.2}  F1 {cl_f:.2}±{cl_f_sd:.2}   pass@{n} {cl_at:.2}  pass^{n} {cl_pow:.2}"
        );

        sum_ep_f1 += ep_f;
        sum_call_f1 += cl_f;
        sum_ep_passpow += ep_pow;
        sum_call_passpow += cl_pow;

        // Non-flaky floor: at least one expected endpoint found in at least one run.
        assert!(
            ep_at > 0.0,
            "[eval] {name}: endpoint pass@k is 0 — nothing matched across {n} run(s)"
        );
    }

    println!(
        "\nMEAN  endpoints F1 {:.2}  pass^k {:.2}   |   calls F1 {:.2}  pass^k {:.2}",
        sum_ep_f1 / nf,
        sum_ep_passpow / nf,
        sum_call_f1 / nf,
        sum_call_passpow / nf
    );
    println!("=== end Tier-A ===\n");
}
