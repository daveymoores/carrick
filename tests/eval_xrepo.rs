//! Live cross-repo accuracy scorer (cross-repo eval S4, #223 — full vector).
//!
//! Scores the *real* scanner over the authored `xrepo-corpus-1` constellation and
//! reports the **full per-metric correctness vector**, **report-only** (a monitor,
//! never a gate). Every metric is the S0 scorer contract §6 row of the same name:
//!
//!   1. **endpoint-set P/R/F1** — `expected.json` producers vs the joined
//!      [`EvalProjection`]'s `endpoints`. HTTP compared as normalized
//!      `(METHOD, norm_path(path))`; GraphQL/socket producers compared on the
//!      canonical `OperationKey` string (which carries kind/direction). Scored
//!      corpus-wide (the joined projection carries no per-repo provenance on ops).
//!   2. **call-set P/R/F1** — `expected.json` consumers vs `EvalProjection.calls`.
//!      HTTP uses the Tier-A fuzzy match (`method ==` AND `host_contains` ⊂ key
//!      AND `path_contains` ⊂ path); GraphQL/socket consumers compare on the
//!      canonical key.
//!   3. **negative `_must_not_emit`** (row 3) — count actual ops whose normalized
//!      `(method, path)` ∈ a repo's `_must_not_emit` list → `decoy_leak`.
//!   4. **owner accuracy** (row 4) — `expected.owner == actual.handler` over
//!      endpoints present in BOTH sets (keyed).
//!   5. **type-anchor accuracy** (row 5) — `expected.primary_type_symbol ==
//!      actual.primary_type_symbol` (null==null ok) over ops in both sets.
//!   6. **type-resolution correctness** (row 6) — `type_state` exact eq AND
//!      whitespace-collapsed resolved-type eq, over ops in both sets that carry an
//!      expected resolved type.
//!   7. **cross-repo match P/R/F1** (row 7) — `expected-output.json.matches` vs
//!      `EvalProjection.cross_repo_matches`, keyed by
//!      `(producer_repo, norm(producer_key), consumer_repo, norm(consumer_key))`,
//!      spanning HTTP + GraphQL + socket edges.
//!   8. **compat-verdict accuracy** (row 8) — `expected.type_compatible ==
//!      actual.type_compatible` over edges in both sets. Guarded by §7: if any
//!      labelled edge expects a non-null verdict, the actual matches MUST contain
//!      ≥1 with `type_compatible.is_some()`, else the scorer **fails loud**.
//!   9. **dependency conflicts** (row 9) — `(package, sorted versions, severity)`
//!      set equality, expected vs actual.
//!  10. **orphans** (row 10) — `(repo, side, norm key)` set equality over expected
//!      orphans vs actual unmatched producers/consumers.
//!
//! Every metric is reported as **mean ± sample sd over N**, **partitioned
//! `capability` vs `roadmap`** (every corpus label is `capability` today, so the
//! `roadmap` partition is empty — the partitioning is wired, not a no-op for the
//! day a `roadmap` label lands). N runs (default 5, `CARRICK_EVAL_RUNS`). One
//! [`EvalRunRecord`] JSONL row (`tier="xrepo"`, `corpus="xrepo-corpus-1"`) is
//! written + echoed for the Axiom history, with the full §6-row vector populated
//! into the S6 record fields.
//!
//! **DEFERRED:** the full monitor cadence (debounced-on-main + issue-on-regression)
//! from #203 is NOT in this slice — it stays a follow-up (build plan §7 slice 4 /
//! #203). This slice ships the metric vector + `workflow_dispatch` only.
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
/// form). For `graphql|<kind>|<field>` and `socket|<dir>|<event>` the third
/// segment has no path params, so `norm_path` is a no-op on it — the function is
/// uniform across protocols.
fn norm_key(key: &str) -> String {
    let mut parts = key.splitn(3, '|');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(proto), Some(method), Some(path)) => {
            format!("{proto}|{}|{}", method.to_uppercase(), norm_path(path))
        }
        _ => key.to_string(),
    }
}

/// Collapse all runs of ASCII whitespace to a single space and trim, for the
/// resolved-type comparison (contract §6 row 6: "whitespace-collapsed resolved
/// -type eq"). Authors and the sidecar disagree only on incidental spacing.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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
// Label shapes (contract §5). The full S4 scorer reads every field. New since
// the thin slice: the `graphql_operations`/`socket_events` arrays (per repo),
// `_must_not_emit`, `owner`/anchor/resolved-type/`type_state`/`tier` on ops,
// and on `expected-output.json` the `orphans` + `dependency_conflicts` arrays
// plus per-`matches` `protocol`/`tier`/compat fields.
// ---------------------------------------------------------------------------

const TIER_CAPABILITY: &str = "capability";
const TIER_ROADMAP: &str = "roadmap";

#[derive(Debug, Deserialize)]
struct ExpectedRepo {
    #[serde(default)]
    endpoints: Vec<ExpEndpoint>,
    #[serde(default)]
    calls: Vec<ExpCall>,
    /// Non-HTTP producer/consumer ops (GraphQL). `role` partitions them into the
    /// endpoint set (producer) vs call set (consumer); `key` is the canonical
    /// `graphql|<kind>|<field>` form.
    #[serde(default)]
    graphql_operations: Vec<ExpNonHttpOp>,
    /// Non-HTTP producer/consumer ops (Socket.IO), same `role`/`key` convention;
    /// `key` is `socket|<direction>|<event>`. Per the corpus README a *listener*
    /// is the producer (endpoint) and an *emitter* the consumer (call).
    #[serde(default)]
    socket_events: Vec<ExpNonHttpOp>,
    #[serde(default, rename = "_must_not_emit")]
    must_not_emit: Vec<ExpMustNotEmit>,
}

#[derive(Debug, Deserialize)]
struct ExpEndpoint {
    method: String,
    path: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    primary_type_symbol: Option<String>,
    #[serde(default)]
    resolved_type: Option<String>,
    #[serde(default)]
    type_state: Option<String>,
    #[serde(default = "default_tier")]
    tier: String,
}

#[derive(Debug, Deserialize)]
struct ExpCall {
    method: String,
    /// Cross-repo match key path (for row 7); set scoring uses the fuzzy fields.
    #[allow(dead_code)]
    path: String,
    #[serde(default)]
    host_contains: Option<String>,
    #[serde(default)]
    path_contains: Option<String>,
    #[serde(default = "default_tier")]
    tier: String,
}

#[derive(Debug, Deserialize)]
struct ExpNonHttpOp {
    /// `"producer"` (→ endpoint set) or `"consumer"` (→ call set).
    role: String,
    /// Canonical `OperationKey` string: `graphql|<kind>|<field>` or
    /// `socket|<direction>|<event>`.
    key: String,
    #[serde(default)]
    primary_type_symbol: Option<String>,
    #[serde(default)]
    resolved_type: Option<String>,
    #[serde(default)]
    type_state: Option<String>,
    #[serde(default = "default_tier")]
    tier: String,
}

#[derive(Debug, Deserialize)]
struct ExpMustNotEmit {
    /// `"endpoint"` or `"call"` — which projection set this decoy would leak into.
    #[allow(dead_code)]
    kind: String,
    /// `"*"` matches any method.
    method: String,
    path: String,
}

fn default_tier() -> String {
    TIER_CAPABILITY.to_string()
}

#[derive(Debug, Deserialize)]
struct ExpectedOutput {
    #[serde(default)]
    matches: Vec<ExpMatch>,
    #[serde(default)]
    orphans: Vec<ExpOrphan>,
    #[serde(default)]
    dependency_conflicts: Vec<ExpDepConflict>,
}

#[derive(Debug, Deserialize)]
struct ExpMatch {
    producer_repo: String,
    producer_key: String,
    consumer_repo: String,
    consumer_key: String,
    /// Expected compat verdict for this edge (row 8). A labelled edge always
    /// carries a non-null verdict (contract §5.2).
    #[serde(default)]
    type_compatible: Option<bool>,
    #[serde(default = "default_tier")]
    tier: String,
}

#[derive(Debug, Deserialize)]
struct ExpOrphan {
    repo: String,
    /// `"producer"` or `"consumer"`.
    side: String,
    /// Canonical `OperationKey` string.
    key: String,
    #[serde(default = "default_tier")]
    tier: String,
}

#[derive(Debug, Deserialize)]
struct ExpDepConflict {
    package: String,
    versions: Vec<String>,
    severity: String,
    #[serde(default = "default_tier")]
    tier: String,
}

// ---------------------------------------------------------------------------
// EvalProjection wire shape. Mirrored locally (not the lib type) so the harness
// reads the projection purely as a wire contract — same stance as the S2 harness.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EvalProjection {
    #[serde(default)]
    endpoints: Vec<EvalOp>,
    #[serde(default)]
    calls: Vec<EvalOp>,
    #[serde(default)]
    cross_repo_matches: Vec<EvalCrossRepoMatch>,
    #[serde(default)]
    dependency_conflicts: Vec<EvalDependencyConflict>,
}

#[derive(Debug, Deserialize, Clone)]
struct EvalOp {
    /// Canonical `OperationKey` string. For GraphQL/socket ops this is the only
    /// place the kind/direction survives (`method` is None, `path` is the bare
    /// field/event), so non-HTTP set scoring keys off this.
    #[serde(default)]
    key: String,
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    handler: Option<String>,
    #[serde(default)]
    type_state: Option<String>,
    #[serde(default)]
    resolved_definition: Option<String>,
    #[serde(default)]
    expanded_definition: Option<String>,
    #[serde(default)]
    primary_type_symbol: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct EvalCrossRepoMatch {
    producer_repo: String,
    producer_key: String,
    consumer_repo: String,
    consumer_key: String,
    #[serde(default)]
    type_compatible: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
struct EvalDependencyConflict {
    package: String,
    versions: Vec<String>,
    severity: String,
}

// ---------------------------------------------------------------------------
// Tier partitioning. Every metric is scored twice — once over the `capability`
// labels, once over the `roadmap` labels — and reported separately (contract §6
// "partitioned by capability vs roadmap, never blended"). The corpus is all
// `capability` today, so the `roadmap` partition is the §1.4 expected-0 perfect
// score until a roadmap label lands.
// ---------------------------------------------------------------------------

const TIERS: [&str; 2] = [TIER_CAPABILITY, TIER_ROADMAP];

// ---------------------------------------------------------------------------
// Pure scoring functions (unit-tested below with synthetic projections).
// ---------------------------------------------------------------------------

/// A normalized op-set member. HTTP ops are keyed `(protocol, METHOD, norm_path)`;
/// GraphQL/socket ops are keyed `(protocol, "", norm_key(canonical))` so the
/// kind/direction the bare `path` drops is preserved. Uniform across protocols.
type OpSetKey = (String, String, String);

/// The expected op-set member for an HTTP endpoint/call label.
fn http_op_key(method: &str, path: &str) -> OpSetKey {
    ("http".to_string(), method.to_uppercase(), norm_path(path))
}

/// The op-set member for a non-HTTP label / projection op, keyed off its
/// canonical operation key (which carries graphql kind / socket direction).
fn nonhttp_op_key(canonical: &str) -> OpSetKey {
    let proto = canonical.split('|').next().unwrap_or("").to_string();
    (proto, String::new(), norm_key(canonical))
}

/// The expected producer op-set for one tier: HTTP `endpoints` + the
/// `role=="producer"` GraphQL/socket ops.
fn expected_endpoint_set(
    repo_expected: &[(String, ExpectedRepo)],
    tier: &str,
) -> HashSet<OpSetKey> {
    let mut set = HashSet::new();
    for (_repo, exp) in repo_expected {
        for e in &exp.endpoints {
            if e.tier == tier {
                set.insert(http_op_key(&e.method, &e.path));
            }
        }
        for op in exp
            .graphql_operations
            .iter()
            .chain(exp.socket_events.iter())
        {
            if op.role == "producer" && op.tier == tier {
                set.insert(nonhttp_op_key(&op.key));
            }
        }
    }
    set
}

/// The projection's producer op-set (HTTP + graphql + socket endpoints).
fn projection_endpoint_set(proj: &EvalProjection) -> HashSet<OpSetKey> {
    proj.endpoints.iter().filter_map(op_to_set_key).collect()
}

/// The projection's consumer op-set (HTTP + graphql + socket calls).
fn projection_call_set(proj: &EvalProjection) -> HashSet<OpSetKey> {
    proj.calls.iter().filter_map(op_to_set_key).collect()
}

/// Project one [`EvalOp`] to its set key. HTTP needs method+path; GraphQL/socket
/// key off the canonical `key`.
fn op_to_set_key(o: &EvalOp) -> Option<OpSetKey> {
    if o.protocol == "http" {
        Some(http_op_key(o.method.as_deref()?, o.path.as_deref()?))
    } else {
        Some(nonhttp_op_key(&o.key))
    }
}

/// Attribute an untiered actual `found` set to one `tier`: keep keys this tier
/// expects, plus true false positives (expected by no tier) attributed to the
/// default tier; drop keys another tier expects. This is the uniform tier
/// -partitioning rule (see [`score_matches`]) applied to the op set, so an
/// all-`capability` corpus leaves the `roadmap` partition's `found` empty.
fn attribute_found_to_tier<T: std::hash::Hash + Eq + Clone>(
    found: &HashSet<T>,
    expected_this: &HashSet<T>,
    expected_any: &HashSet<T>,
    expected_other: &HashSet<T>,
    tier: &str,
) -> HashSet<T> {
    found
        .iter()
        .filter(|k| {
            expected_this.contains(*k)
                || (!expected_other.contains(*k)
                    && !expected_any.contains(*k)
                    && tier == TIER_CAPABILITY)
        })
        .cloned()
        .collect()
}

/// Endpoint-set P/R/F1 for one tier (contract §6 row 1).
fn score_endpoint_set(
    repo_expected: &[(String, ExpectedRepo)],
    proj: &EvalProjection,
    tier: &str,
) -> (f64, f64, f64) {
    let expected = expected_endpoint_set(repo_expected, tier);
    let expected_any: HashSet<OpSetKey> = TIERS
        .iter()
        .flat_map(|t| expected_endpoint_set(repo_expected, t))
        .collect();
    let expected_other: HashSet<OpSetKey> = TIERS
        .iter()
        .filter(|t| **t != tier)
        .flat_map(|t| expected_endpoint_set(repo_expected, t))
        .collect();
    let found_all = projection_endpoint_set(proj);
    let found =
        attribute_found_to_tier(&found_all, &expected, &expected_any, &expected_other, tier);
    let tp = expected.intersection(&found).count();
    prf(tp, found.len(), expected.len())
}

/// Call-set P/R/F1 for one tier (contract §6 row 2). HTTP calls use the Tier-A
/// fuzzy match (method == AND host_contains ⊂ key AND path_contains ⊂ path);
/// GraphQL/socket consumers use exact canonical-key set membership. Returns
/// `(precision, recall, f1)` blending both protocol families into one row.
fn score_call_set(
    repo_expected: &[(String, ExpectedRepo)],
    proj: &EvalProjection,
    tier: &str,
) -> (f64, f64, f64) {
    // --- HTTP, fuzzy (Tier-A convention) ---
    let http_calls: Vec<&EvalOp> = proj.calls.iter().filter(|o| o.protocol == "http").collect();
    let expected_http: Vec<&ExpCall> = repo_expected
        .iter()
        .flat_map(|(_r, e)| e.calls.iter())
        .filter(|c| c.tier == tier)
        .collect();
    let expected_http_other: Vec<&ExpCall> = repo_expected
        .iter()
        .flat_map(|(_r, e)| e.calls.iter())
        .filter(|c| c.tier != tier)
        .collect();
    // recall: every expected call matched by ≥1 actual.
    let http_recall_hits = expected_http
        .iter()
        .filter(|ec| http_calls.iter().any(|ac| fuzzy_call_match(ec, ac)))
        .count();
    // precision: every actual call that satisfies ≥1 expected (over this tier).
    let http_precision_hits = http_calls
        .iter()
        .filter(|ac| expected_http.iter().any(|ec| fuzzy_call_match(ec, ac)))
        .count();
    // Tier attribution of the http `found` denominator: an actual claimed by
    // ANOTHER tier's expected call is dropped from this tier's denominator; a
    // true false positive (matches no tier) lands in the default tier. So an
    // all-capability corpus leaves roadmap's http `found` empty.
    let http_found = http_calls
        .iter()
        .filter(|ac| {
            let matches_this = expected_http.iter().any(|ec| fuzzy_call_match(ec, ac));
            let matches_other = expected_http_other
                .iter()
                .any(|ec| fuzzy_call_match(ec, ac));
            matches_this || (!matches_other && tier == TIER_CAPABILITY)
        })
        .count();

    // --- GraphQL/socket consumers, exact canonical key ---
    let nonhttp_expected = |t: &str| -> HashSet<OpSetKey> {
        repo_expected
            .iter()
            .flat_map(|(_r, e)| e.graphql_operations.iter().chain(e.socket_events.iter()))
            .filter(|op| op.role == "consumer" && op.tier == t)
            .map(|op| nonhttp_op_key(&op.key))
            .collect()
    };
    let expected_nonhttp = nonhttp_expected(tier);
    let expected_nonhttp_any: HashSet<OpSetKey> =
        TIERS.iter().flat_map(|t| nonhttp_expected(t)).collect();
    let expected_nonhttp_other: HashSet<OpSetKey> = TIERS
        .iter()
        .filter(|t| **t != tier)
        .flat_map(|t| nonhttp_expected(t))
        .collect();
    let found_nonhttp_all: HashSet<OpSetKey> = proj
        .calls
        .iter()
        .filter(|o| o.protocol != "http")
        .map(|o| nonhttp_op_key(&o.key))
        .collect();
    let found_nonhttp = attribute_found_to_tier(
        &found_nonhttp_all,
        &expected_nonhttp,
        &expected_nonhttp_any,
        &expected_nonhttp_other,
        tier,
    );
    let nonhttp_tp = expected_nonhttp.intersection(&found_nonhttp).count();

    // Blend: P over (http precision-eligible actuals + nonhttp actuals),
    // R over (http expected + nonhttp expected).
    let found = http_found + found_nonhttp.len();
    let expected = expected_http.len() + expected_nonhttp.len();
    let tp_precision = http_precision_hits + nonhttp_tp;
    let tp_recall = http_recall_hits + nonhttp_tp;
    let precision = if found == 0 {
        if expected == 0 { 1.0 } else { 0.0 }
    } else {
        tp_precision as f64 / found as f64
    };
    let recall = if expected == 0 {
        1.0
    } else {
        tp_recall as f64 / expected as f64
    };
    let f1 = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };
    (precision, recall, f1)
}

/// Tier-A fuzzy call match: same method, expected `host_contains` is a substring
/// of the actual op's canonical key, and expected `path_contains` is a substring
/// of the actual path. A null `host_contains`/`path_contains` constraint is
/// vacuously satisfied (it is a "no host" / built-in label, e.g. `sendBeacon`).
fn fuzzy_call_match(ec: &ExpCall, ac: &EvalOp) -> bool {
    let method_ok = ac
        .method
        .as_deref()
        .map(|m| m.eq_ignore_ascii_case(&ec.method))
        .unwrap_or(false);
    if !method_ok {
        return false;
    }
    let host_ok = match &ec.host_contains {
        Some(h) => ac.key.contains(h.as_str()),
        None => true,
    };
    let path_ok = match &ec.path_contains {
        Some(p) => ac
            .path
            .as_deref()
            .map(|ap| ap.contains(p.as_str()))
            .unwrap_or(false),
        None => true,
    };
    host_ok && path_ok
}

/// Decoy leakage (contract §6 row 3): the count of actual ops (endpoints +
/// calls) whose normalized `(method, path)` equals a `_must_not_emit` entry.
/// `method: "*"` matches any method. Reported separately, not tier-partitioned
/// (a leak is a leak regardless of which tier the surrounding labels are).
fn score_decoy_leak(repo_expected: &[(String, ExpectedRepo)], proj: &EvalProjection) -> usize {
    let decoys: Vec<&ExpMustNotEmit> = repo_expected
        .iter()
        .flat_map(|(_r, e)| e.must_not_emit.iter())
        .collect();
    let mut leaks = 0usize;
    for op in proj.endpoints.iter().chain(proj.calls.iter()) {
        let (Some(m), Some(p)) = (op.method.as_deref(), op.path.as_deref()) else {
            // Non-HTTP ops have no (method, path); the corpus decoys are all
            // HTTP-keyed, so a non-HTTP op can never match one.
            continue;
        };
        let m_up = m.to_uppercase();
        let p_norm = norm_path(p);
        let is_leak = decoys.iter().any(|d| {
            let method_ok = d.method == "*" || d.method.eq_ignore_ascii_case(&m_up);
            method_ok && norm_path(&d.path) == p_norm
        });
        if is_leak {
            leaks += 1;
        }
    }
    leaks
}

/// Index the projection's HTTP endpoints by `(METHOD, norm_path)` and its
/// non-HTTP endpoints by canonical key, so owner/type metrics can join expected
/// to actual on the shared key.
fn index_proj_endpoints(proj: &EvalProjection) -> std::collections::HashMap<OpSetKey, &EvalOp> {
    let mut idx = std::collections::HashMap::new();
    for o in &proj.endpoints {
        if let Some(k) = op_to_set_key(o) {
            idx.entry(k).or_insert(o);
        }
    }
    idx
}

/// Index the projection's consumer calls by key, for the consumer-side type
/// metrics (GraphQL/socket consumers + HTTP calls carry anchors/resolved types).
fn index_proj_calls(proj: &EvalProjection) -> std::collections::HashMap<OpSetKey, &EvalOp> {
    let mut idx = std::collections::HashMap::new();
    for o in &proj.calls {
        if let Some(k) = op_to_set_key(o) {
            idx.entry(k).or_insert(o);
        }
    }
    idx
}

/// `(correct, total)` fraction with the convention `(0,0) -> 1.0` (nothing to
/// score is vacuously perfect, matching the §1.4 expected-0 stance).
fn fraction(correct: usize, total: usize) -> f64 {
    if total == 0 {
        1.0
    } else {
        correct as f64 / total as f64
    }
}

/// Owner accuracy (contract §6 row 4): over endpoints present in BOTH the
/// expected and actual sets, the fraction where `expected.owner == actual.handler`.
/// Only HTTP endpoints carry `owner`/`handler`. `null == null` counts correct.
fn score_owner_accuracy(
    repo_expected: &[(String, ExpectedRepo)],
    proj: &EvalProjection,
    tier: &str,
) -> f64 {
    let idx = index_proj_endpoints(proj);
    let mut correct = 0usize;
    let mut total = 0usize;
    for (_r, exp) in repo_expected {
        for e in &exp.endpoints {
            if e.tier != tier {
                continue;
            }
            let key = http_op_key(&e.method, &e.path);
            if let Some(actual) = idx.get(&key) {
                total += 1;
                if e.owner.as_deref() == actual.handler.as_deref() {
                    correct += 1;
                }
            }
        }
    }
    fraction(correct, total)
}

/// Type-anchor accuracy (contract §6 row 5): over ops present in BOTH sets
/// (endpoints + calls, all protocols), the fraction where
/// `expected.primary_type_symbol == actual.primary_type_symbol` (`null==null` ok).
fn score_type_anchor_accuracy(
    repo_expected: &[(String, ExpectedRepo)],
    proj: &EvalProjection,
    tier: &str,
) -> f64 {
    let ep_idx = index_proj_endpoints(proj);
    let call_idx = index_proj_calls(proj);
    let mut correct = 0usize;
    let mut total = 0usize;
    for_each_expected_typed_op(repo_expected, tier, |key, is_producer, anchor, _rt, _ts| {
        let idx = if is_producer { &ep_idx } else { &call_idx };
        if let Some(actual) = idx.get(&key) {
            total += 1;
            if anchor == actual.primary_type_symbol.as_deref() {
                correct += 1;
            }
        }
    });
    fraction(correct, total)
}

/// Type-resolution correctness (contract §6 row 6): over ops in both sets that
/// carry an expected resolved type, the fraction where `type_state` is exactly
/// equal AND the whitespace-collapsed resolved type is equal. The actual
/// resolved type is `resolved_definition`, falling back to `expanded_definition`.
fn score_type_resolution_accuracy(
    repo_expected: &[(String, ExpectedRepo)],
    proj: &EvalProjection,
    tier: &str,
) -> f64 {
    let ep_idx = index_proj_endpoints(proj);
    let call_idx = index_proj_calls(proj);
    let mut correct = 0usize;
    let mut total = 0usize;
    for_each_expected_typed_op(repo_expected, tier, |key, is_producer, _anchor, rt, ts| {
        let Some(expected_rt) = rt else {
            return; // only score ops with an expected resolved type
        };
        let idx = if is_producer { &ep_idx } else { &call_idx };
        if let Some(actual) = idx.get(&key) {
            total += 1;
            let actual_rt = actual
                .resolved_definition
                .as_deref()
                .or(actual.expanded_definition.as_deref());
            let state_ok = ts == actual.type_state.as_deref();
            let rt_ok = actual_rt.map(collapse_ws) == Some(collapse_ws(expected_rt));
            if state_ok && rt_ok {
                correct += 1;
            }
        }
    });
    fraction(correct, total)
}

/// Drive a closure over every expected typed op of `tier`: HTTP endpoints (as
/// producers), GraphQL/socket producers (endpoints) and consumers (calls).
/// `is_producer` selects which projection index the caller joins against. Args:
/// `(set_key, is_producer, anchor, resolved_type, type_state)`.
fn for_each_expected_typed_op(
    repo_expected: &[(String, ExpectedRepo)],
    tier: &str,
    mut f: impl FnMut(OpSetKey, bool, Option<&str>, Option<&str>, Option<&str>),
) {
    for (_r, exp) in repo_expected {
        for e in &exp.endpoints {
            if e.tier == tier {
                f(
                    http_op_key(&e.method, &e.path),
                    true,
                    e.primary_type_symbol.as_deref(),
                    e.resolved_type.as_deref(),
                    e.type_state.as_deref(),
                );
            }
        }
        for op in exp
            .graphql_operations
            .iter()
            .chain(exp.socket_events.iter())
        {
            if op.tier == tier {
                let is_producer = op.role == "producer";
                f(
                    nonhttp_op_key(&op.key),
                    is_producer,
                    op.primary_type_symbol.as_deref(),
                    op.resolved_type.as_deref(),
                    op.type_state.as_deref(),
                );
            }
        }
    }
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

/// Cross-repo match P/R/F1 for one tier (contract §6 row 7): exact set equality
/// over the edge key, both sides normalized. Spans HTTP + GraphQL + socket edges.
///
/// **Tier attribution of actuals.** The contract partitions every metric by
/// tier, but the scanner's actual edges carry no tier. An actual edge is
/// attributed to a tier iff it matches an expected edge of that tier, OR it
/// matches no expected edge of ANY tier (a true false positive, attributed to
/// the *default* tier so a spurious edge still dings precision exactly once). So
/// a tier's `found` set excludes actuals claimed by another tier — an
/// all-`capability` corpus then leaves the `roadmap` partition's `found` empty
/// (no roadmap false positives), scoring the §1.4 expected-0 perfect.
fn score_matches(
    expected: &[ExpMatch],
    found: &[EvalCrossRepoMatch],
    tier: &str,
) -> (f64, f64, f64) {
    let edge_key = |m: &ExpMatch| {
        match_edge_key(
            &m.producer_repo,
            &m.producer_key,
            &m.consumer_repo,
            &m.consumer_key,
        )
    };
    let expected_set: HashSet<_> = expected
        .iter()
        .filter(|m| m.tier == tier)
        .map(edge_key)
        .collect();
    // Keys expected by any tier (to detect true false positives) and by some
    // OTHER tier (to exclude them from this tier's `found`).
    let expected_any: HashSet<_> = expected.iter().map(edge_key).collect();
    let expected_other: HashSet<_> = expected
        .iter()
        .filter(|m| m.tier != tier)
        .map(edge_key)
        .collect();
    let found_set: HashSet<_> = found
        .iter()
        .map(|m| {
            match_edge_key(
                &m.producer_repo,
                &m.producer_key,
                &m.consumer_repo,
                &m.consumer_key,
            )
        })
        // Attribute a true false positive (matches no expected edge anywhere) to
        // the default tier; drop edges claimed by another tier.
        .filter(|k| {
            expected_set.contains(k)
                || (!expected_other.contains(k)
                    && (!expected_any.contains(k) && tier == TIER_CAPABILITY))
        })
        .collect();
    let tp = expected_set.intersection(&found_set).count();
    prf(tp, found_set.len(), expected_set.len())
}

/// Compat-verdict accuracy for one tier (contract §6 row 8): over edges present
/// in BOTH expected and actual `matches` (keyed §6 row 7), the fraction where
/// `actual.type_compatible == Some(expected.type_compatible)`. A labelled edge
/// always carries a non-null verdict, so an actual `None` is a miss. The §7
/// guard is enforced separately by [`compat_guard`] before this is trusted.
fn score_compat_verdict_accuracy(
    expected: &[ExpMatch],
    found: &[EvalCrossRepoMatch],
    tier: &str,
) -> f64 {
    use std::collections::HashMap;
    let actual_by_key: HashMap<(String, String, String, String), Option<bool>> = found
        .iter()
        .map(|m| {
            (
                match_edge_key(
                    &m.producer_repo,
                    &m.producer_key,
                    &m.consumer_repo,
                    &m.consumer_key,
                ),
                m.type_compatible,
            )
        })
        .collect();
    let mut correct = 0usize;
    let mut total = 0usize;
    for m in expected.iter().filter(|m| m.tier == tier) {
        let key = match_edge_key(
            &m.producer_repo,
            &m.producer_key,
            &m.consumer_repo,
            &m.consumer_key,
        );
        if let Some(actual_compat) = actual_by_key.get(&key) {
            total += 1;
            if *actual_compat == m.type_compatible && m.type_compatible.is_some() {
                correct += 1;
            }
        }
    }
    fraction(correct, total)
}

/// The §7 `ts_check_dir` guard (load-bearing). If the answer key expects ≥1 edge
/// to carry a non-null compat verdict, the actual matches MUST contain ≥1 edge
/// with `type_compatible.is_some()` — otherwise compat data was silently absent
/// (a forgotten `ts_check/`) and the scorer must FAIL LOUD rather than score the
/// absence as "all compatible". Returns `Err(msg)` to fail; `Ok(())` to proceed.
fn compat_guard(expected: &ExpectedOutput, found: &[EvalCrossRepoMatch]) -> Result<(), String> {
    let expects_verdict = expected.matches.iter().any(|m| m.type_compatible.is_some());
    if !expects_verdict {
        return Ok(());
    }
    let has_verdict = found.iter().any(|m| m.type_compatible.is_some());
    if has_verdict {
        Ok(())
    } else {
        Err(format!(
            "§7 ts_check_dir guard: {} expected edge(s) carry a non-null compat verdict, but \
             NO actual cross_repo_match has type_compatible.is_some(). Cross-repo type checking \
             was silently skipped (a forgotten ts_check/ dir). Refusing to score absent compat \
             data as a verdict.",
            expected
                .matches
                .iter()
                .filter(|m| m.type_compatible.is_some())
                .count()
        ))
    }
}

/// Dependency-conflict P/R/F1 for one tier (contract §6 row 9): exact set
/// equality over `(package, sorted versions, severity)`. Tier attribution of the
/// untiered actuals mirrors [`score_matches`]: an actual conflict is scored under
/// a tier iff it equals a labelled conflict of that tier, or it equals no
/// labelled conflict of ANY tier (a true false positive, attributed to the
/// default tier). So an all-`capability` corpus leaves the `roadmap` partition's
/// `found` empty → §1.4 expected-0 perfect.
fn score_dep_conflicts(
    expected: &[ExpDepConflict],
    found: &[EvalDependencyConflict],
    tier: &str,
) -> (f64, f64, f64) {
    let dep_key = |pkg: &str, versions: &[String], severity: &str| {
        let mut v = versions.to_vec();
        v.sort();
        (pkg.to_string(), v, severity.to_string())
    };
    let expected_set: HashSet<(String, Vec<String>, String)> = expected
        .iter()
        .filter(|d| d.tier == tier)
        .map(|d| dep_key(&d.package, &d.versions, &d.severity))
        .collect();
    let expected_any: HashSet<_> = expected
        .iter()
        .map(|d| dep_key(&d.package, &d.versions, &d.severity))
        .collect();
    let expected_other: HashSet<_> = expected
        .iter()
        .filter(|d| d.tier != tier)
        .map(|d| dep_key(&d.package, &d.versions, &d.severity))
        .collect();
    let found_set: HashSet<(String, Vec<String>, String)> = found
        .iter()
        .map(|d| dep_key(&d.package, &d.versions, &d.severity))
        .filter(|k| {
            expected_set.contains(k)
                || (!expected_other.contains(k)
                    && (!expected_any.contains(k) && tier == TIER_CAPABILITY))
        })
        .collect();
    let tp = expected_set.intersection(&found_set).count();
    prf(tp, found_set.len(), expected_set.len())
}

/// Orphans P/R/F1 for one tier (contract §6 row 10, optional). An expected orphan
/// is a producer/consumer that the matcher SHOULD leave unmatched. The joined
/// projection has no per-repo provenance on its `endpoints`/`calls` (the row-1
/// TODO), so the actual orphan universe is not directly enumerable; the only
/// observable signal is the matched edges. An expected orphan is "confirmed" iff
/// its `(repo, side, norm key)` does NOT appear among the matched edges' endpoints
/// — i.e. the matcher correctly did not match it. A labelled orphan the matcher
/// *did* match is a recall miss (the more dangerous error: a spurious edge). So
/// recall is the metric that bites here; precision is definitionally 1.0 over the
/// observable set. Reported, never asserted. When the projection gains repo
/// provenance on ops, swap to the direct producer/consumer minus matched-edge
/// difference for a true precision too.
fn score_orphans(
    expected: &[ExpOrphan],
    matches: &[EvalCrossRepoMatch],
    tier: &str,
) -> (f64, f64, f64) {
    let matched: HashSet<(String, String, String)> = matches
        .iter()
        .flat_map(|m| {
            [
                (
                    m.producer_repo.clone(),
                    "producer".to_string(),
                    norm_key(&m.producer_key),
                ),
                (
                    m.consumer_repo.clone(),
                    "consumer".to_string(),
                    norm_key(&m.consumer_key),
                ),
            ]
        })
        .collect();
    let expected_orphans: HashSet<(String, String, String)> = expected
        .iter()
        .filter(|o| o.tier == tier)
        .map(|o| (o.repo.clone(), o.side.clone(), norm_key(&o.key)))
        .collect();
    let tp = expected_orphans
        .iter()
        .filter(|o| !matched.contains(*o))
        .count();
    prf(tp, tp, expected_orphans.len())
}

/// One run's full metric vector, per tier.
#[derive(Clone)]
struct TierScore {
    ep_prf: (f64, f64, f64),
    call_prf: (f64, f64, f64),
    match_prf: (f64, f64, f64),
    owner: f64,
    type_anchor: f64,
    type_resolution: f64,
    compat_verdict: f64,
    dep_prf: (f64, f64, f64),
    orphan_prf: (f64, f64, f64),
}

#[derive(Clone)]
struct RunScore {
    /// Keyed by tier name (`capability` / `roadmap`).
    by_tier: std::collections::HashMap<String, TierScore>,
    /// Decoy leakage (not tier-partitioned).
    decoy_leak: usize,
}

/// Score one joined projection against the corpus labels. Enforces the §7 compat
/// guard before computing the compat-verdict metric — failing loud if compat data
/// is silently absent.
fn score_corpus(
    proj: &EvalProjection,
    repo_expected: &[(String, ExpectedRepo)],
    expected_output: &ExpectedOutput,
) -> Result<RunScore, String> {
    compat_guard(expected_output, &proj.cross_repo_matches)?;
    let mut by_tier = std::collections::HashMap::new();
    for tier in TIERS {
        by_tier.insert(
            tier.to_string(),
            TierScore {
                ep_prf: score_endpoint_set(repo_expected, proj, tier),
                call_prf: score_call_set(repo_expected, proj, tier),
                match_prf: score_matches(&expected_output.matches, &proj.cross_repo_matches, tier),
                owner: score_owner_accuracy(repo_expected, proj, tier),
                type_anchor: score_type_anchor_accuracy(repo_expected, proj, tier),
                type_resolution: score_type_resolution_accuracy(repo_expected, proj, tier),
                compat_verdict: score_compat_verdict_accuracy(
                    &expected_output.matches,
                    &proj.cross_repo_matches,
                    tier,
                ),
                dep_prf: score_dep_conflicts(
                    &expected_output.dependency_conflicts,
                    &proj.dependency_conflicts,
                    tier,
                ),
                orphan_prf: score_orphans(&expected_output.orphans, &proj.cross_repo_matches, tier),
            },
        );
    }
    Ok(RunScore {
        by_tier,
        decoy_leak: score_decoy_leak(repo_expected, proj),
    })
}

// ---------------------------------------------------------------------------
// EvalRunRecord — the longitudinal store row. Same schema as Tier-A's record
// (the cross-repo fields were added S6); the full S4 scorer populates the whole
// vector. The capability-tier numbers ride the record's scalar/Option columns
// (the corpus is all-capability today); roadmap is reported in-console.
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
    // Call-set metric (contract §6 row 2) rides the `call_*` columns.
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
    // --- cross-repo facets (the full S4 vector) ---
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
/// checking *runs* and the compat-verdict metric is scored over real data. Fails
/// loud if type checking was silently skipped (a missing `ts_check/`), per §7.
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
    // Some. Fail loud if it was silently skipped — the compat-verdict metric must
    // never be built on silently-absent compat data.
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

/// Mean±sd of one extracted f64 across the per-run scores, for a given tier.
fn agg(scores: &[RunScore], tier: &str, pick: impl Fn(&TierScore) -> f64) -> (f64, f64) {
    let xs: Vec<f64> = scores
        .iter()
        .filter_map(|s| s.by_tier.get(tier).map(&pick))
        .collect();
    mean_sd(&xs)
}

/// Print the full metric vector for one tier (report-only).
fn report_tier(scores: &[RunScore], tier: &str, n: usize, runs_n: usize) {
    let (ep_p, ep_p_sd) = agg(scores, tier, |t| t.ep_prf.0);
    let (ep_r, ep_r_sd) = agg(scores, tier, |t| t.ep_prf.1);
    let (ep_f, ep_f_sd) = agg(scores, tier, |t| t.ep_prf.2);
    let (cl_p, cl_p_sd) = agg(scores, tier, |t| t.call_prf.0);
    let (cl_r, cl_r_sd) = agg(scores, tier, |t| t.call_prf.1);
    let (cl_f, cl_f_sd) = agg(scores, tier, |t| t.call_prf.2);
    let (m_p, m_p_sd) = agg(scores, tier, |t| t.match_prf.0);
    let (m_r, m_r_sd) = agg(scores, tier, |t| t.match_prf.1);
    let (m_f, m_f_sd) = agg(scores, tier, |t| t.match_prf.2);
    let (own, own_sd) = agg(scores, tier, |t| t.owner);
    let (anc, anc_sd) = agg(scores, tier, |t| t.type_anchor);
    let (res, res_sd) = agg(scores, tier, |t| t.type_resolution);
    let (cmp, cmp_sd) = agg(scores, tier, |t| t.compat_verdict);
    let (dp_f, dp_f_sd) = agg(scores, tier, |t| t.dep_prf.2);
    let (orph_f, orph_f_sd) = agg(scores, tier, |t| t.orphan_prf.2);

    println!("\n{CORPUS} — tier={tier} (n={n}/{runs_n})");
    println!(
        "  endpoint-set   P {ep_p:.2}±{ep_p_sd:.2}  R {ep_r:.2}±{ep_r_sd:.2}  F1 {ep_f:.2}±{ep_f_sd:.2}"
    );
    println!(
        "  call-set       P {cl_p:.2}±{cl_p_sd:.2}  R {cl_r:.2}±{cl_r_sd:.2}  F1 {cl_f:.2}±{cl_f_sd:.2}"
    );
    println!(
        "  xrepo match    P {m_p:.2}±{m_p_sd:.2}  R {m_r:.2}±{m_r_sd:.2}  F1 {m_f:.2}±{m_f_sd:.2}"
    );
    println!("  owner accuracy        {own:.2}±{own_sd:.2}");
    println!("  type-anchor accuracy  {anc:.2}±{anc_sd:.2}");
    println!("  type-resolution acc   {res:.2}±{res_sd:.2}");
    println!("  compat-verdict acc    {cmp:.2}±{cmp_sd:.2}");
    println!("  dependency F1         {dp_f:.2}±{dp_f_sd:.2}");
    println!("  orphans F1            {orph_f:.2}±{orph_f_sd:.2}");
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

    println!("\n=== Cross-repo live scorer ({CORPUS}, N={runs_n}) — full S4 vector ===");
    println!("(report-only monitor; the only fail-loud is the §7 compat-absence guard)\n");

    let mut scores: Vec<RunScore> = Vec::new();
    for run_idx in 1..=runs_n {
        let proj = run_two_phase(&bin, &corpus, false);
        // Extraction diagnostic (report-only, run 1 only). A cross-repo match
        // metric of 0 (or endpoint recall < 1) is otherwise opaque: surface the
        // ENDPOINT SET, the calls, and the edges so a miss is attributable — which
        // producer endpoint failed to extract, what URL the consumer called, and
        // which repo identities the edges carry.
        if run_idx == 1 {
            let found_eps = projection_endpoint_set(&proj);
            let expected_eps = expected_endpoint_set(&repo_expected, TIER_CAPABILITY);
            eprintln!(
                "[diag] run 1 projection: {} endpoints, {} calls, {} cross_repo_matches, \
                 {} dependency_conflicts",
                proj.endpoints.len(),
                proj.calls.len(),
                proj.cross_repo_matches.len(),
                proj.dependency_conflicts.len()
            );
            eprintln!(
                "[diag] endpoint set: {} expected (capability), {} found",
                expected_eps.len(),
                found_eps.len()
            );
            for e in &expected_eps {
                let mark = if found_eps.contains(e) { "ok " } else { "MISS" };
                eprintln!("[diag]   endpoint[{mark}] {}|{}|{}", e.0, e.1, e.2);
            }
            for e in &found_eps {
                if !expected_eps.contains(e) {
                    eprintln!("[diag]   endpoint[XTRA] {}|{}|{}", e.0, e.1, e.2);
                }
            }
            let found_calls = projection_call_set(&proj);
            eprintln!(
                "[diag] call set: {} found (distinct keys)",
                found_calls.len()
            );
            for c in &proj.calls {
                eprintln!(
                    "[diag]   call {} {} (key {})",
                    c.method.clone().unwrap_or_default(),
                    c.path.clone().unwrap_or_default(),
                    c.key
                );
            }
            for m in &proj.cross_repo_matches {
                eprintln!(
                    "[diag]   edge {}|{} -> {}|{} (compat={:?})",
                    m.producer_repo,
                    m.producer_key,
                    m.consumer_repo,
                    m.consumer_key,
                    m.type_compatible
                );
            }
        }
        let score = score_corpus(&proj, &repo_expected, &expected_output)
            .unwrap_or_else(|e| panic!("[eval] §7 compat guard tripped: {e}"));
        let cap = score.by_tier.get(TIER_CAPABILITY).expect("capability tier");
        println!(
            "  run {run_idx}/{runs_n}: endpoint F1 {:.2}  call F1 {:.2}  match F1 {:.2}  \
             anchor {:.2}  resolution {:.2}  compat {:.2}  dep F1 {:.2}  decoy_leak {}",
            cap.ep_prf.2,
            cap.call_prf.2,
            cap.match_prf.2,
            cap.type_anchor,
            cap.type_resolution,
            cap.compat_verdict,
            cap.dep_prf.2,
            score.decoy_leak,
        );
        scores.push(score);
    }
    let n = scores.len();
    assert!(n > 0, "[eval] every run failed");

    report_tier(&scores, TIER_CAPABILITY, n, runs_n);
    report_tier(&scores, TIER_ROADMAP, n, runs_n);

    let (decoy_mean, decoy_sd) = mean_sd(
        &scores
            .iter()
            .map(|s| s.decoy_leak as f64)
            .collect::<Vec<_>>(),
    );
    println!("\n  decoy_leak (corpus-wide, lower is better)  {decoy_mean:.2}±{decoy_sd:.2}");
    println!("=== end cross-repo live scorer (report-only; only the §7 guard fails) ===\n");

    // The record rides the capability-tier numbers (corpus is all-capability).
    let (ep_p, ep_p_sd) = agg(&scores, TIER_CAPABILITY, |t| t.ep_prf.0);
    let (ep_r, ep_r_sd) = agg(&scores, TIER_CAPABILITY, |t| t.ep_prf.1);
    let (ep_f, ep_f_sd) = agg(&scores, TIER_CAPABILITY, |t| t.ep_prf.2);
    let (cl_p, cl_p_sd) = agg(&scores, TIER_CAPABILITY, |t| t.call_prf.0);
    let (cl_r, cl_r_sd) = agg(&scores, TIER_CAPABILITY, |t| t.call_prf.1);
    let (cl_f, cl_f_sd) = agg(&scores, TIER_CAPABILITY, |t| t.call_prf.2);
    let (m_p, m_p_sd) = agg(&scores, TIER_CAPABILITY, |t| t.match_prf.0);
    let (m_r, m_r_sd) = agg(&scores, TIER_CAPABILITY, |t| t.match_prf.1);
    let (m_f, m_f_sd) = agg(&scores, TIER_CAPABILITY, |t| t.match_prf.2);
    let (own, own_sd) = agg(&scores, TIER_CAPABILITY, |t| t.owner);
    let (anc, anc_sd) = agg(&scores, TIER_CAPABILITY, |t| t.type_anchor);
    let (res, res_sd) = agg(&scores, TIER_CAPABILITY, |t| t.type_resolution);
    let (cmp, cmp_sd) = agg(&scores, TIER_CAPABILITY, |t| t.compat_verdict);
    let (dp_f, dp_f_sd) = agg(&scores, TIER_CAPABILITY, |t| t.dep_prf.2);

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
        call_precision_mean: cl_p,
        call_precision_sd: cl_p_sd,
        call_recall_mean: cl_r,
        call_recall_sd: cl_r_sd,
        call_f1_mean: cl_f,
        call_f1_sd: cl_f_sd,
        call_pass_at_k: 0.0,
        call_pass_pow_k: 0.0,
        note: Some(
            "S4 full vector: endpoint/call/match P/R/F1 + owner/anchor/resolution/compat + \
             deps/orphans/decoys (capability tier)"
                .to_string(),
        ),
        tier: Some("xrepo".to_string()),
        corpus: Some(CORPUS.to_string()),
        match_precision_mean: Some(m_p),
        match_precision_sd: Some(m_p_sd),
        match_recall_mean: Some(m_r),
        match_recall_sd: Some(m_r_sd),
        match_f1_mean: Some(m_f),
        match_f1_sd: Some(m_f_sd),
        type_anchor_accuracy_mean: Some(anc),
        type_anchor_accuracy_sd: Some(anc_sd),
        type_resolution_accuracy_mean: Some(res),
        type_resolution_accuracy_sd: Some(res_sd),
        compat_verdict_accuracy_mean: Some(cmp),
        compat_verdict_accuracy_sd: Some(cmp_sd),
        owner_accuracy_mean: Some(own),
        owner_accuracy_sd: Some(own_sd),
        decoy_leak_mean: Some(decoy_mean),
        decoy_leak_sd: Some(decoy_sd),
        dep_f1_mean: Some(dp_f),
        dep_f1_sd: Some(dp_f_sd),
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
// known numbers. They run via the ci.yml `eval_xrepo` step.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod scoring_tests {
    use super::*;

    fn http_ep(method: &str, path: &str) -> EvalOp {
        EvalOp {
            key: format!("http|{}|{}", method.to_uppercase(), path),
            protocol: "http".to_string(),
            method: Some(method.to_string()),
            path: Some(path.to_string()),
            handler: None,
            type_state: None,
            resolved_definition: None,
            expanded_definition: None,
            primary_type_symbol: None,
        }
    }

    fn nonhttp_op(protocol: &str, key: &str) -> EvalOp {
        // path = the bare third segment (field/event), as the projection emits.
        let path = key.split('|').nth(2).map(str::to_string);
        EvalOp {
            key: key.to_string(),
            protocol: protocol.to_string(),
            method: None,
            path,
            handler: None,
            type_state: None,
            resolved_definition: None,
            expanded_definition: None,
            primary_type_symbol: None,
        }
    }

    fn cm(p_repo: &str, p_key: &str, c_repo: &str, c_key: &str) -> EvalCrossRepoMatch {
        EvalCrossRepoMatch {
            producer_repo: p_repo.to_string(),
            producer_key: p_key.to_string(),
            consumer_repo: c_repo.to_string(),
            consumer_key: c_key.to_string(),
            type_compatible: None,
        }
    }

    fn cm_compat(
        p_repo: &str,
        p_key: &str,
        c_repo: &str,
        c_key: &str,
        compat: Option<bool>,
    ) -> EvalCrossRepoMatch {
        EvalCrossRepoMatch {
            producer_repo: p_repo.to_string(),
            producer_key: p_key.to_string(),
            consumer_repo: c_repo.to_string(),
            consumer_key: c_key.to_string(),
            type_compatible: compat,
        }
    }

    fn empty_proj() -> EvalProjection {
        EvalProjection {
            endpoints: vec![],
            calls: vec![],
            cross_repo_matches: vec![],
            dependency_conflicts: vec![],
        }
    }

    // --- Shared primitives ---

    #[test]
    fn norm_path_collapses_all_three_param_syntaxes() {
        assert_eq!(norm_path("/orders/:id"), "/orders/:param");
        assert_eq!(norm_path("/orders/{id}"), "/orders/:param");
        assert_eq!(norm_path("/orders/[id]"), "/orders/:param");
        assert_eq!(norm_path("/orders/"), "/orders");
        assert_eq!(norm_path("/"), "/");
    }

    #[test]
    fn norm_key_uppercases_method_and_normalizes_path() {
        assert_eq!(norm_key("http|get|/orders/{id}"), "http|GET|/orders/:param");
        assert_eq!(norm_key("http|GET|/orders/:id"), "http|GET|/orders/:param");
        // GraphQL/socket keys: the third segment has no path params → unchanged
        // apart from the (no-op) uppercasing of the already-cased middle segment.
        assert_eq!(norm_key("graphql|query|order"), "graphql|QUERY|order");
        assert_eq!(
            norm_key("socket|SERVER->CLIENT|payment:settled"),
            "socket|SERVER->CLIENT|payment:settled"
        );
        assert_eq!(norm_key("not-a-key"), "not-a-key");
    }

    #[test]
    fn collapse_ws_normalizes_whitespace() {
        assert_eq!(
            collapse_ws("{ id:  number;\n  x: string }"),
            "{ id: number; x: string }"
        );
        assert_eq!(collapse_ws("  a  b  "), "a b");
    }

    #[test]
    fn prf_contract_conventions() {
        assert_eq!(prf(0, 0, 0), (1.0, 1.0, 1.0));
        assert_eq!(prf(0, 2, 0).0, 0.0);
        assert_eq!(prf(3, 3, 3), (1.0, 1.0, 1.0));
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
        assert!((sd - 0.5_f64.sqrt()).abs() < 1e-9);
        assert_eq!(mean_sd(&[]), (0.0, 0.0));
    }

    // --- Synthetic-projection metric tests ---

    #[test]
    fn nonhttp_op_key_keeps_kind_and_direction() {
        // The bare path drops kind/direction; the canonical key keeps it, so the
        // op-set key must derive from `key`, not `(method,path)`.
        let q = nonhttp_op("graphql", "graphql|query|order");
        let s = nonhttp_op("graphql", "graphql|subscription|order"); // same field, diff kind
        assert_ne!(op_to_set_key(&q), op_to_set_key(&s));
        let listen = nonhttp_op("socket", "socket|SERVER->CLIENT|payment:settled");
        let emit = nonhttp_op("socket", "socket|CLIENT->SERVER|payment:settled");
        assert_ne!(op_to_set_key(&listen), op_to_set_key(&emit));
    }

    #[test]
    fn match_set_exact_with_normalization() {
        let expected = vec![
            ExpMatch {
                producer_repo: "orders-pkg".to_string(),
                producer_key: "http|GET|/orders/:param".to_string(),
                consumer_repo: "payments-svc".to_string(),
                consumer_key: "http|GET|/orders/:param".to_string(),
                type_compatible: Some(true),
                tier: TIER_CAPABILITY.to_string(),
            },
            ExpMatch {
                producer_repo: "payments-svc".to_string(),
                producer_key: "http|POST|/payments".to_string(),
                consumer_repo: "web-frontend".to_string(),
                consumer_key: "http|POST|/payments".to_string(),
                type_compatible: Some(true),
                tier: TIER_CAPABILITY.to_string(),
            },
        ];
        // The found edge uses {id}/[id] + lowercased method → normalizes to match
        // the first expected edge; the second is missing.
        let found = vec![cm(
            "orders-pkg",
            "http|get|/orders/{id}",
            "payments-svc",
            "http|GET|/orders/[id]",
        )];
        let (p, r, _f) = score_matches(&expected, &found, TIER_CAPABILITY);
        assert_eq!(p, 1.0, "the one found edge matched after normalization");
        assert_eq!(r, 0.5, "one of two expected edges found");
    }

    #[test]
    fn match_set_spans_graphql_and_socket() {
        let expected = vec![
            ExpMatch {
                producer_repo: "gateway".to_string(),
                producer_key: "graphql|query|order".to_string(),
                consumer_repo: "web-frontend".to_string(),
                consumer_key: "graphql|query|order".to_string(),
                type_compatible: Some(true),
                tier: TIER_CAPABILITY.to_string(),
            },
            ExpMatch {
                producer_repo: "web-frontend".to_string(),
                producer_key: "socket|SERVER->CLIENT|payment:settled".to_string(),
                consumer_repo: "payments-svc".to_string(),
                consumer_key: "socket|SERVER->CLIENT|payment:settled".to_string(),
                type_compatible: Some(true),
                tier: TIER_CAPABILITY.to_string(),
            },
        ];
        let found = vec![
            cm(
                "gateway",
                "graphql|query|order",
                "web-frontend",
                "graphql|query|order",
            ),
            cm(
                "web-frontend",
                "socket|SERVER->CLIENT|payment:settled",
                "payments-svc",
                "socket|SERVER->CLIENT|payment:settled",
            ),
        ];
        let (p, r, f) = score_matches(&expected, &found, TIER_CAPABILITY);
        assert_eq!((p, r, f), (1.0, 1.0, 1.0), "both non-HTTP edges matched");
    }

    #[test]
    fn decoy_leak_counts_must_not_emit_hits() {
        let repos = vec![(
            "payments-svc".to_string(),
            serde_json::from_value::<ExpectedRepo>(serde_json::json!({
                "_must_not_emit": [
                    { "kind": "call", "method": "*", "path": "/__sdk__/PutItemCommand" },
                    { "kind": "endpoint", "method": "POST", "path": "/__lambda__/settle" }
                ]
            }))
            .unwrap(),
        )];
        let mut proj = empty_proj();
        proj.calls
            .push(http_ep("DELETE", "/__sdk__/PutItemCommand")); // leak (method *)
        proj.endpoints.push(http_ep("POST", "/__lambda__/settle")); // leak
        proj.endpoints.push(http_ep("GET", "/__lambda__/settle")); // NOT a leak (wrong method)
        proj.endpoints.push(http_ep("GET", "/orders/1")); // clean
        assert_eq!(score_decoy_leak(&repos, &proj), 2);
    }

    #[test]
    fn owner_accuracy_over_intersection() {
        let repos = vec![(
            "orders-pkg".to_string(),
            serde_json::from_value::<ExpectedRepo>(serde_json::json!({
                "endpoints": [
                    { "method": "GET", "path": "/orders/:id", "owner": "ordersRouter", "tier": "capability" },
                    { "method": "GET", "path": "/gateway/health", "owner": "healthCheckHandler", "tier": "capability" }
                ]
            }))
            .unwrap(),
        )];
        let mut proj = empty_proj();
        let mut ok = http_ep("GET", "/orders/:id");
        ok.handler = Some("ordersRouter".to_string());
        let mut fab = http_ep("GET", "/gateway/health");
        fab.handler = Some("GET".to_string()); // owner=method fabrication
        proj.endpoints.push(ok);
        proj.endpoints.push(fab);
        // 2 in both sets, 1 correct.
        assert!((score_owner_accuracy(&repos, &proj, TIER_CAPABILITY) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn type_anchor_accuracy_null_equals_null() {
        let repos = vec![(
            "gateway".to_string(),
            serde_json::from_value::<ExpectedRepo>(serde_json::json!({
                "endpoints": [
                    { "method": "GET", "path": "/users/:id", "primary_type_symbol": "UserSummary", "tier": "capability" },
                    { "method": "GET", "path": "/users/recent", "primary_type_symbol": null, "tier": "capability" }
                ]
            }))
            .unwrap(),
        )];
        let mut proj = empty_proj();
        let mut a = http_ep("GET", "/users/:id");
        a.primary_type_symbol = Some("UserSummary".to_string());
        let b = http_ep("GET", "/users/recent"); // anchor None == expected null
        proj.endpoints.push(a);
        proj.endpoints.push(b);
        assert!((score_type_anchor_accuracy(&repos, &proj, TIER_CAPABILITY) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn type_resolution_state_and_whitespace() {
        let repos = vec![(
            "orders-pkg".to_string(),
            serde_json::from_value::<ExpectedRepo>(serde_json::json!({
                "endpoints": [
                    { "method": "GET", "path": "/orders/:id",
                      "resolved_type": "{ id: number; amountCents: number; }",
                      "type_state": "Explicit", "tier": "capability" },
                    { "method": "GET", "path": "/users/recent",
                      "resolved_type": "{ count: number; ids: string[]; }",
                      "type_state": "Implicit", "tier": "capability" }
                ]
            }))
            .unwrap(),
        )];
        let mut proj = empty_proj();
        let mut a = http_ep("GET", "/orders/:id");
        // Same type, different spacing → whitespace-collapsed eq.
        a.resolved_definition = Some("{ id: number;   amountCents: number; }".to_string());
        a.type_state = Some("Explicit".to_string());
        let mut b = http_ep("GET", "/users/recent");
        b.resolved_definition = Some("{ count: number; ids: string[]; }".to_string());
        b.type_state = Some("Explicit".to_string()); // WRONG state (should be Implicit)
        proj.endpoints.push(a);
        proj.endpoints.push(b);
        // a correct, b wrong (state mismatch) → 0.5.
        assert!(
            (score_type_resolution_accuracy(&repos, &proj, TIER_CAPABILITY) - 0.5).abs() < 1e-9
        );
    }

    #[test]
    fn compat_verdict_accuracy_and_none_is_miss() {
        let expected = vec![
            ExpMatch {
                producer_repo: "orders-pkg".to_string(),
                producer_key: "http|GET|/orders/:param".to_string(),
                consumer_repo: "payments-svc".to_string(),
                consumer_key: "http|GET|/orders/:param".to_string(),
                type_compatible: Some(true),
                tier: TIER_CAPABILITY.to_string(),
            },
            ExpMatch {
                producer_repo: "orders-pkg".to_string(),
                producer_key: "http|GET|/orders/:param".to_string(),
                consumer_repo: "web-frontend".to_string(),
                consumer_key: "http|GET|/orders/:param".to_string(),
                type_compatible: Some(false),
                tier: TIER_CAPABILITY.to_string(),
            },
        ];
        let found = vec![
            cm_compat(
                "orders-pkg",
                "http|GET|/orders/:param",
                "payments-svc",
                "http|GET|/orders/:param",
                Some(true), // correct
            ),
            cm_compat(
                "orders-pkg",
                "http|GET|/orders/:param",
                "web-frontend",
                "http|GET|/orders/:param",
                None, // actual None on a labelled edge → miss
            ),
        ];
        // 2 edges in both sets, 1 verdict correct.
        assert!(
            (score_compat_verdict_accuracy(&expected, &found, TIER_CAPABILITY) - 0.5).abs() < 1e-9
        );
    }

    #[test]
    fn compat_guard_fails_when_verdict_absent() {
        let expected: ExpectedOutput = serde_json::from_value(serde_json::json!({
            "matches": [
                { "producer_repo": "a", "producer_key": "http|GET|/x",
                  "consumer_repo": "b", "consumer_key": "http|GET|/x",
                  "type_compatible": true, "tier": "capability" }
            ]
        }))
        .unwrap();
        // No actual edge carries a verdict → guard trips.
        let absent = vec![cm("a", "http|GET|/x", "b", "http|GET|/x")];
        assert!(compat_guard(&expected, &absent).is_err());
        // At least one verdict present → guard passes.
        let present = vec![cm_compat(
            "a",
            "http|GET|/x",
            "b",
            "http|GET|/x",
            Some(true),
        )];
        assert!(compat_guard(&expected, &present).is_ok());
        // No edge expects a verdict → guard vacuously passes.
        let no_expect: ExpectedOutput = serde_json::from_value(serde_json::json!({
            "matches": [
                { "producer_repo": "a", "producer_key": "http|GET|/x",
                  "consumer_repo": "b", "consumer_key": "http|GET|/x", "tier": "capability" }
            ]
        }))
        .unwrap();
        assert!(compat_guard(&no_expect, &absent).is_ok());
    }

    #[test]
    fn dep_conflicts_exact_set() {
        let expected = vec![ExpDepConflict {
            package: "zod".to_string(),
            versions: vec!["3.22.0".to_string(), "4.0.0".to_string()],
            severity: "critical".to_string(),
            tier: TIER_CAPABILITY.to_string(),
        }];
        // Found out-of-order versions normalize to a match.
        let found = vec![EvalDependencyConflict {
            package: "zod".to_string(),
            versions: vec!["4.0.0".to_string(), "3.22.0".to_string()],
            severity: "critical".to_string(),
        }];
        assert_eq!(
            score_dep_conflicts(&expected, &found, TIER_CAPABILITY),
            (1.0, 1.0, 1.0)
        );
        // Wrong severity → no match.
        let wrong = vec![EvalDependencyConflict {
            package: "zod".to_string(),
            versions: vec!["3.22.0".to_string(), "4.0.0".to_string()],
            severity: "warning".to_string(),
        }];
        let (p, r, _) = score_dep_conflicts(&expected, &wrong, TIER_CAPABILITY);
        assert_eq!((p, r), (0.0, 0.0));
    }

    #[test]
    fn orphans_recall_over_matched_complement() {
        let expected = vec![
            ExpOrphan {
                repo: "orders-pkg".to_string(),
                side: "producer".to_string(),
                key: "http|GET|/api/v1/status".to_string(),
                tier: TIER_CAPABILITY.to_string(),
            },
            ExpOrphan {
                repo: "payments-svc".to_string(),
                side: "consumer".to_string(),
                key: "http|POST|/billing/charge".to_string(),
                tier: TIER_CAPABILITY.to_string(),
            },
        ];
        // The matcher matched /billing/charge (so it is NOT actually an orphan →
        // recall miss); /api/v1/status stays unmatched → correct orphan.
        let matches = vec![cm(
            "orders-pkg",
            "http|POST|/billing",
            "payments-svc",
            "http|POST|/billing/charge",
        )];
        let (_p, r, _f) = score_orphans(&expected, &matches, TIER_CAPABILITY);
        assert!(
            (r - 0.5).abs() < 1e-9,
            "1 of 2 labelled orphans confirmed unmatched"
        );
    }

    #[test]
    fn roadmap_tier_empty_scores_perfect() {
        // No roadmap labels anywhere → every per-tier metric is the §1.4
        // expected-0 perfect score (nothing expected, nothing scored).
        let repos: Vec<(String, ExpectedRepo)> = vec![];
        let proj = empty_proj();
        let out: ExpectedOutput = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(
            score_endpoint_set(&repos, &proj, TIER_ROADMAP),
            (1.0, 1.0, 1.0)
        );
        assert_eq!(score_call_set(&repos, &proj, TIER_ROADMAP), (1.0, 1.0, 1.0));
        assert_eq!(score_owner_accuracy(&repos, &proj, TIER_ROADMAP), 1.0);
        assert_eq!(
            score_dep_conflicts(
                &out.dependency_conflicts,
                &proj.dependency_conflicts,
                TIER_ROADMAP
            ),
            (1.0, 1.0, 1.0)
        );
    }

    // --- Real-corpus-label tests (deterministic; load the committed labels) ---

    #[test]
    fn perfect_synthetic_projection_against_real_corpus() {
        let corpus = corpus_dir();
        let repos = discover_repos(&corpus);
        assert_eq!(repos.len(), 3, "the 3 top-level corpus repos");
        let repo_expected = load_repo_expected(&repos);
        let expected_output = load_expected_output(&corpus);

        // Build a projection whose endpoint/call sets equal the union of all
        // repos' expected ops, whose matches equal the expected edges (with their
        // labelled compat verdict), and whose deps equal the labelled conflict.
        let mut endpoints: Vec<EvalOp> = Vec::new();
        let mut calls: Vec<EvalOp> = Vec::new();
        for (_repo, exp) in &repo_expected {
            for e in &exp.endpoints {
                let mut op = http_ep(&e.method, &e.path);
                op.handler = e.owner.clone();
                op.primary_type_symbol = e.primary_type_symbol.clone();
                op.resolved_definition = e.resolved_type.clone();
                op.type_state = e.type_state.clone();
                endpoints.push(op);
            }
            for c in &exp.calls {
                // Mirror the consumer call so the fuzzy match fires: key carries
                // the host token, path carries the path token.
                let host = c.host_contains.clone().unwrap_or_default();
                let path = c.path.clone();
                let mut op = http_ep(&c.method, &path);
                op.key = format!("http|{}|{}/{}", c.method.to_uppercase(), host, path);
                op.path = Some(path);
                calls.push(op);
            }
            for op in exp
                .graphql_operations
                .iter()
                .chain(exp.socket_events.iter())
            {
                let proto = op.key.split('|').next().unwrap_or("");
                let mut eo = nonhttp_op(proto, &op.key);
                eo.primary_type_symbol = op.primary_type_symbol.clone();
                eo.resolved_definition = op.resolved_type.clone();
                eo.type_state = op.type_state.clone();
                if op.role == "producer" {
                    endpoints.push(eo);
                } else {
                    calls.push(eo);
                }
            }
        }
        let cross_repo_matches: Vec<EvalCrossRepoMatch> = expected_output
            .matches
            .iter()
            .map(|m| {
                cm_compat(
                    &m.producer_repo,
                    &m.producer_key,
                    &m.consumer_repo,
                    &m.consumer_key,
                    m.type_compatible,
                )
            })
            .collect();
        let dependency_conflicts: Vec<EvalDependencyConflict> = expected_output
            .dependency_conflicts
            .iter()
            .map(|d| EvalDependencyConflict {
                package: d.package.clone(),
                versions: d.versions.clone(),
                severity: d.severity.clone(),
            })
            .collect();
        let proj = EvalProjection {
            endpoints,
            calls,
            cross_repo_matches,
            dependency_conflicts,
        };

        let score = score_corpus(&proj, &repo_expected, &expected_output)
            .expect("§7 guard passes — the perfect projection carries verdicts");
        let cap = score.by_tier.get(TIER_CAPABILITY).unwrap();
        assert_eq!(cap.ep_prf, (1.0, 1.0, 1.0), "perfect endpoint set");
        assert_eq!(cap.call_prf.1, 1.0, "perfect call recall");
        assert_eq!(cap.match_prf, (1.0, 1.0, 1.0), "perfect match set");
        assert!((cap.owner - 1.0).abs() < 1e-9, "perfect owner");
        assert!((cap.type_anchor - 1.0).abs() < 1e-9, "perfect anchor");
        assert!(
            (cap.type_resolution - 1.0).abs() < 1e-9,
            "perfect resolution"
        );
        assert!((cap.compat_verdict - 1.0).abs() < 1e-9, "perfect compat");
        assert_eq!(cap.dep_prf, (1.0, 1.0, 1.0), "perfect deps");
        assert!(
            (cap.orphan_prf.1 - 1.0).abs() < 1e-9,
            "perfect orphan recall"
        );
        assert_eq!(score.decoy_leak, 0, "perfect projection emits no decoys");
        // roadmap tier is empty everywhere → perfect by the expected-0 convention.
        let road = score.by_tier.get(TIER_ROADMAP).unwrap();
        assert_eq!(road.ep_prf, (1.0, 1.0, 1.0));
        assert_eq!(road.match_prf, (1.0, 1.0, 1.0));
    }

    #[test]
    fn corpus_has_the_expected_protocol_and_edge_shape() {
        // Pins the corpus invariants the scorer relies on, so a label edit that
        // drifts the shape fails here (answer-key-drift discipline).
        let corpus = corpus_dir();
        let repos = discover_repos(&corpus);
        let repo_expected = load_repo_expected(&repos);
        let expected_output = load_expected_output(&corpus);

        // 6 matched edges: 3 http + 2 graphql + 1 socket.
        assert_eq!(expected_output.matches.len(), 6);
        let n_proto = |p: &str| {
            expected_output
                .matches
                .iter()
                .filter(|m| m.producer_key.starts_with(&format!("{p}|")))
                .count()
        };
        assert_eq!(n_proto("http"), 3, "3 HTTP edges");
        assert_eq!(n_proto("graphql"), 2, "2 GraphQL edges");
        assert_eq!(n_proto("socket"), 1, "1 socket edge");

        // 2 incompatible edges (one REST, one GraphQL subscription).
        let incompatible = expected_output
            .matches
            .iter()
            .filter(|m| m.type_compatible == Some(false))
            .count();
        assert_eq!(incompatible, 2, "2 deliberately-incompatible edges");

        // Every edge carries a non-null verdict → the §7 guard is live.
        assert!(
            expected_output
                .matches
                .iter()
                .all(|m| m.type_compatible.is_some()),
            "every corpus edge labels a compat verdict"
        );

        // The corpus authors the first Implicit type_state (gateway /users/recent).
        let any_implicit = repo_expected.iter().any(|(_r, e)| {
            e.endpoints
                .iter()
                .any(|ep| ep.type_state.as_deref() == Some("Implicit"))
        });
        assert!(
            any_implicit,
            "corpus has its first Implicit type_state case"
        );

        // Decoys are present and total 4 (2 MCP + audit SDK + lambda settle).
        let n_decoys: usize = repo_expected
            .iter()
            .map(|(_r, e)| e.must_not_emit.len())
            .sum();
        assert_eq!(n_decoys, 4, "4 _must_not_emit decoys across the corpus");

        // Dependency conflict: one cross-repo zod 3.x vs 4.x, critical.
        assert_eq!(expected_output.dependency_conflicts.len(), 1);
        let dc = &expected_output.dependency_conflicts[0];
        assert_eq!(dc.package, "zod");
        assert_eq!(dc.severity, "critical");
    }

    #[test]
    fn empty_projection_against_real_corpus_scores_zero_recall() {
        let corpus = corpus_dir();
        let repos = discover_repos(&corpus);
        let repo_expected = load_repo_expected(&repos);
        let expected_output = load_expected_output(&corpus);

        // The empty projection has no compat verdicts, so the §7 guard trips —
        // that is the correct loud failure (compat data is absent). Verify the
        // guard, then score over a projection that DOES carry a verdict so the
        // other metrics' zero-recall behaviour is observable.
        let empty = empty_proj();
        assert!(
            score_corpus(&empty, &repo_expected, &expected_output).is_err(),
            "empty projection → §7 guard trips (no compat verdict present)"
        );

        // A projection with one (irrelevant) verdict-bearing edge clears the
        // guard; everything else is still empty → recall 0 on the real metrics.
        let mut proj = empty_proj();
        proj.cross_repo_matches.push(cm_compat(
            "x",
            "http|GET|/nope",
            "y",
            "http|GET|/nope",
            Some(true),
        ));
        let score = score_corpus(&proj, &repo_expected, &expected_output).unwrap();
        let cap = score.by_tier.get(TIER_CAPABILITY).unwrap();
        assert_eq!(cap.ep_prf.1, 0.0, "no endpoints found → recall 0");
        assert_eq!(cap.ep_prf.0, 0.0, "found==0 with expected>0 → precision 0");
        assert_eq!(cap.match_prf.1, 0.0, "no real matches found → recall 0");
        assert_eq!(cap.dep_prf.1, 0.0, "no deps found → recall 0");
    }

    #[test]
    fn eval_run_record_round_trips_full_vector() {
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
            call_precision_mean: 0.9,
            call_precision_sd: 0.05,
            call_recall_mean: 0.85,
            call_recall_sd: 0.06,
            call_f1_mean: 0.87,
            call_f1_sd: 0.05,
            call_pass_at_k: 0.0,
            call_pass_pow_k: 0.0,
            note: Some("S4 full vector".into()),
            tier: Some("xrepo".into()),
            corpus: Some(CORPUS.into()),
            match_precision_mean: Some(0.6),
            match_precision_sd: Some(0.2),
            match_recall_mean: Some(0.5),
            match_recall_sd: Some(0.15),
            match_f1_mean: Some(0.55),
            match_f1_sd: Some(0.17),
            type_anchor_accuracy_mean: Some(0.9),
            type_anchor_accuracy_sd: Some(0.0),
            type_resolution_accuracy_mean: Some(0.8),
            type_resolution_accuracy_sd: Some(0.0),
            compat_verdict_accuracy_mean: Some(1.0),
            compat_verdict_accuracy_sd: Some(0.0),
            owner_accuracy_mean: Some(0.75),
            owner_accuracy_sd: Some(0.0),
            decoy_leak_mean: Some(0.0),
            decoy_leak_sd: Some(0.0),
            dep_f1_mean: Some(1.0),
            dep_f1_sd: Some(0.0),
        };
        let line = serde_json::to_string(&rec).unwrap();
        let back: EvalRunRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(back.tier.as_deref(), Some("xrepo"));
        assert_eq!(back.corpus.as_deref(), Some(CORPUS));
        assert_eq!(back.match_f1_mean, Some(0.55));
        // The full vector now serializes (it is no longer None/omitted).
        assert_eq!(back.compat_verdict_accuracy_mean, Some(1.0));
        assert_eq!(back.type_anchor_accuracy_mean, Some(0.9));
        assert_eq!(back.dep_f1_mean, Some(1.0));
        assert!(line.contains("compat_verdict_accuracy_mean"));
        assert!(line.contains("dep_f1_mean"));
    }
}
