//! Tier-A extraction-quality scorer (evals plan, Slice 1).
//!
//! Runs the release scanner against each framework fixture with
//! `CARRICK_OUTPUT_JSON=1`, scores endpoint & call precision/recall/F1 against
//! the fixture's `expected.json`, and prints a report. **Report-only:** it
//! asserts only a non-flaky floor (valid JSON + endpoint recall > 0), because the
//! live LLM is nondeterministic and a strict assertion would flake.
//!
//! Auth is GitHub Actions OIDC — the scanner is keyless (no API-key path). The
//! runner exposes `ACTIONS_ID_TOKEN_REQUEST_URL`/`_TOKEN` only when the job has
//! `permissions: id-token: write`, and the scanner exchanges them for an
//! `X-Carrick-OIDC` token. When OIDC is unavailable (e.g. local `cargo test`)
//! this test skips cleanly. Runner: `.github/workflows/eval-tier-a.yml`.
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

/// Run the scanner in JSON mode, retrying once on empty stdout (a known
/// transient when the cloud/LLM hiccups). The OIDC env
/// (`ACTIONS_ID_TOKEN_REQUEST_URL`/`_TOKEN`) is inherited from the runner; the
/// API endpoint is compile-time (`build.rs`), not a runtime var.
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
        // Surface the exit status + tail of stderr so a CI failure is
        // diagnosable rather than a bare "empty output" panic.
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

    println!("\n=== Tier-A extraction quality (evals Slice 1, N=1) ===");
    println!(
        "{:<16} {:>24} {:>24}",
        "fixture", "endpoints P/R/F1", "calls P/R/F1"
    );

    let n = FIXTURES.len() as f64;
    let (mut sep, mut scall) = ((0.0, 0.0, 0.0), (0.0, 0.0, 0.0));

    for name in FIXTURES {
        let fixture_dir = manifest_dir().join("tests/fixtures").join(name);
        let expected: Expected = serde_json::from_str(
            &std::fs::read_to_string(fixture_dir.join("expected.json"))
                .unwrap_or_else(|e| panic!("read expected.json for {name}: {e}")),
        )
        .unwrap_or_else(|e| panic!("parse expected.json for {name}: {e}"));

        let stdout = run_scanner(&bin, &fixture_dir).unwrap_or_else(|| {
            panic!("[eval] {name}: empty output after retry (cloud/LLM hiccup)")
        });
        let projection = parse_projection(&stdout).unwrap_or_else(|| {
            panic!("[eval] {name}: stdout was not valid EvalProjection JSON:\n{stdout}")
        });

        // Endpoints: set comparison over (METHOD, normalised path), HTTP only.
        let expected_eps: HashSet<(String, String)> = expected
            .endpoints
            .iter()
            .map(|e| (e.method.to_uppercase(), norm_path(&e.path)))
            .collect();
        let found_eps: HashSet<(String, String)> = projection
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
        let ep_tp = expected_eps.intersection(&found_eps).count();
        let (ep_p, ep_r, ep_f1) = prf(ep_tp, found_eps.len(), expected_eps.len());

        // Calls: fuzzy match on method + host_contains + path_contains, because a
        // consumer's target is a raw URL/expression, not a normalised route.
        let found_calls: Vec<(String, String)> = projection
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
        let matched_expected = expected
            .calls
            .iter()
            .filter(|e| found_calls.iter().any(|c| matches(e, c)))
            .count();
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

        println!(
            "{:<16} {:>6.2}/{:>5.2}/{:>5.2} {:>8.2}/{:>5.2}/{:>5.2}",
            name, ep_p, ep_r, ep_f1, call_p, call_r, call_f1
        );

        sep = (sep.0 + ep_p, sep.1 + ep_r, sep.2 + ep_f1);
        scall = (scall.0 + call_p, scall.1 + call_r, scall.2 + call_f1);

        // Non-flaky floor: parsed JSON + at least one correct endpoint.
        assert!(
            ep_r > 0.0,
            "[eval] {name}: endpoint recall is 0 — found {found_eps:?}, expected {expected_eps:?}"
        );
    }

    println!(
        "{:<16} {:>6.2}/{:>5.2}/{:>5.2} {:>8.2}/{:>5.2}/{:>5.2}",
        "MEAN",
        sep.0 / n,
        sep.1 / n,
        sep.2 / n,
        scall.0 / n,
        scall.1 / n,
        scall.2 / n
    );
    println!("=== end Tier-A ===\n");
}
