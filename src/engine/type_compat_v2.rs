//! v2 type-compat integration ("tsc as serializer / tsc as judge").
//!
//! Scan time: derive capture anchors from the SAME collected type requests
//! the v1 bundle path uses (byte-identical aliases; the alias contracts in
//! `file_orchestrator` are load-bearing and untouched), run `capture_v2`
//! through the sidecar's stdio seam, and store the resulting stub package on
//! `CloudRepoData` as the wire artifact.
//!
//! Check time: materialize every participating service's stub, build one
//! `CheckPairSpec` per matched (producer, consumer, type_kind) manifest pair
//! (porting the ts_check manifest-matcher semantics: method + route-aware
//! path match for HTTP, exact operation keys for socket/graphql/pubsub),
//! run `check_v2`, and map the four-bucket verdicts back onto pair outcomes
//! the analyzer joins to `CrossRepoMatch` edges by structured identity.
//! No verdict travels as a parsed human label anywhere on this path.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::analyzer::PairCheckOutcome;
use crate::cloud_storage::{
    CAPTURE_ARTIFACT_VERSION, CaptureStubArtifact, CloudRepoData, ManifestRole, ManifestTypeKind,
    ManifestTypeState, TypeManifestEntry,
};
use crate::operation::OperationKey;
use crate::services::TypeSidecar;
use crate::services::type_sidecar::{
    AnchorOrigin, CaptureAnchor, CheckPairEndpoint, CheckPairSpec, CheckStubInput,
    InferRequestItem, ProbeProtocol, ProbeTypeKind, SymbolRequest, VerdictBucket,
};

// ===========================================================================
// Scan time: capture
// ===========================================================================

/// Reduce a source path to its repo-root-relative form for the capture wire
/// (the sidecar joins `source_file` onto `repo_root`). Absolute paths under
/// the root are stripped; anything else passes through unchanged.
fn repo_relative(file_path: &str, repo_root: &str) -> String {
    let root = repo_root.trim_end_matches('/');
    let stripped = if root.is_empty() || root == "." {
        file_path
    } else {
        file_path
            .strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(file_path)
    };
    stripped.strip_prefix("./").unwrap_or(stripped).to_string()
}

/// One shared notion of a "disqualifying top type" in printed TypeScript
/// type text (adversarial-review finding 2, aligned with the capture
/// self-check's deep walk): `any` or `unknown` appearing as a TYPE token at
/// ANY position — the whole text, an element (`any[]`), a type argument
/// (`Promise<any>`, `Record<string, any>`), a member
/// (`{ metadata: any }`), or an index signature (`{ [k: string]: any }`).
/// Such text must never anchor a literal capture or count as a resolved
/// shape: `any` is bidirectionally assignable, so an arbitrary counterparty
/// shape would read compatible, and `unknown` is the scrubbers' failed-
/// inference placeholder.
///
/// String-literal types are stripped first (`{ kind: "any" }` is fine) and
/// property-NAME positions are excluded (`{ any: string }`, `{ any?: T }`).
/// The error direction is deliberate: a false positive only demotes toward
/// unverifiable; a false negative is a false-compatible.
pub(crate) fn contains_disqualifying_top_type(text: &str) -> bool {
    let scrubbed = strip_string_literal_contents(text);
    let bytes = scrubbed.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    for keyword in ["any", "unknown"] {
        let mut search_from = 0;
        while let Some(pos) = scrubbed[search_from..].find(keyword) {
            let begin = search_from + pos;
            let end = begin + keyword.len();
            search_from = begin + 1;
            // Identifier boundaries (`company`, `unknownField`, `Anything`).
            if begin > 0 && is_ident(bytes[begin - 1]) {
                continue;
            }
            if end < bytes.len() && is_ident(bytes[end]) {
                continue;
            }
            // Property-name position: printers emit `name: T` / `name?: T`
            // with the colon immediately after the name.
            let rest = &bytes[end..];
            let rest = if rest.first() == Some(&b'?') {
                &rest[1..]
            } else {
                rest
            };
            if rest.first() == Some(&b':') {
                continue;
            }
            return true;
        }
    }
    false
}

/// Replace the CONTENTS of string-literal types with nothing, keeping the
/// quotes, so `{ kind: "any" }` cannot false-positive the token scan.
fn strip_string_literal_contents(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '\'' || c == '"' || c == '`' {
            let quote = c;
            let mut escaped = false;
            for inner in chars.by_ref() {
                if escaped {
                    escaped = false;
                    continue;
                }
                if inner == '\\' {
                    escaped = true;
                } else if inner == quote {
                    out.push(quote);
                    break;
                }
            }
        }
    }
    out
}

/// A v1 inferred type text is usable as a literal anchor when it carries an
/// actual shape: not the unknown/any placeholders the scrubbers leave, and
/// not container-decayed text (`any[]`, `Promise<any>`, `Record<string,
/// any>`, `{ metadata: any }`) — a rejected text falls back to the
/// locator-based infer anchor, whose result the capture self-check owns.
fn usable_inferred_text(text: &str) -> Option<&str> {
    let trimmed = text.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() || contains_disqualifying_top_type(trimmed) {
        None
    } else {
        Some(trimmed)
    }
}

/// Derive one capture anchor per alias from the collected v1 type requests.
///
/// Precedence mirrors the v1 bundle: an explicit symbol request wins over an
/// infer request for the same alias, which wins over an inline literal. The
/// alias strings are consumed as-is — this function never re-derives or
/// rewrites an alias, so the manifest join keys stay byte-identical.
///
/// For an infer-request alias, the v1 inference RESULT (when it produced a
/// real shape) rides a literal anchor at the structural_fallback tier: the
/// v1 inferrer is kind-aware (payload args, params, wrapper unwrapping via
/// the extraction config), which the capture-native locator is not yet — a
/// raw locator re-run resolves the wrong node for exactly those cases
/// (e.g. a `bus.emit(...)` boolean instead of its payload). The tier keeps
/// the legacy-text dependence measured and ratchetable; the locator-based
/// infer anchor remains the path for aliases v1 inference could not resolve.
///
/// `anchor_origin` mapping is pragmatic for WP3: symbol/literal anchors are
/// LLM-sourced (`llm-symbol`), infer-derived anchors are locator-driven
/// (`deterministic-infer`). Refining backfill attribution is follow-up work.
pub(crate) fn derive_capture_anchors(
    explicit: &[SymbolRequest],
    infer: &[InferRequestItem],
    inline_aliases: &[(String, String)],
    inferred: &[crate::services::type_sidecar::InferredType],
    repo_root: &str,
) -> Vec<CaptureAnchor> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut anchors: Vec<CaptureAnchor> = Vec::new();

    // First usable inferred text per alias (mirrors the enrich join's
    // first-wins `or_insert`).
    let mut inferred_text: HashMap<&str, &str> = HashMap::new();
    for inf in inferred {
        if let Some(text) = usable_inferred_text(&inf.type_string) {
            inferred_text.entry(inf.alias.as_str()).or_insert(text);
        }
    }

    for request in explicit {
        let Some(alias) = request.alias.as_deref() else {
            // No alias means no manifest entry to join; nothing to capture.
            continue;
        };
        if !seen.insert(alias.to_string()) {
            continue;
        }
        anchors.push(CaptureAnchor::Symbol {
            alias: alias.to_string(),
            symbol_name: request.symbol_name.clone(),
            source_file: repo_relative(&request.source_file, repo_root),
            anchor_origin: AnchorOrigin::LlmSymbol,
            array_depth: request.array_depth.filter(|d| *d > 0),
        });
    }

    for request in infer {
        let Some(alias) = request.alias.as_deref() else {
            continue;
        };
        if !seen.insert(alias.to_string()) {
            continue;
        }
        // Kind-aware v1 inference result wins over a raw locator re-run.
        if let Some(text) = inferred_text.get(alias) {
            anchors.push(CaptureAnchor::Literal {
                alias: alias.to_string(),
                type_text: (*text).to_string(),
                anchor_origin: AnchorOrigin::DeterministicInfer,
            });
            continue;
        }
        // The capture locator prefers span, then expression text (from its
        // line), then first expression on the line — same precedence as the
        // v1 inferrer's locator inputs.
        let line_number = request
            .expression_line
            .filter(|l| *l > 0)
            .or(Some(request.line_number))
            .filter(|l| *l > 0);
        anchors.push(CaptureAnchor::Infer {
            alias: alias.to_string(),
            source_file: repo_relative(&request.file_path, repo_root),
            anchor_origin: AnchorOrigin::DeterministicInfer,
            span_start: request.span_start,
            span_end: request.span_end,
            line_number,
            expression_text: request.expression_text.clone(),
        });
    }

    for (alias, type_text) in inline_aliases {
        if type_text.trim().is_empty() || !seen.insert(alias.clone()) {
            continue;
        }
        anchors.push(CaptureAnchor::Literal {
            alias: alias.clone(),
            type_text: type_text.clone(),
            anchor_origin: AnchorOrigin::LlmSymbol,
        });
    }

    anchors
}

/// Run `capture_v2` for one service and read the stub package into the wire
/// artifact. Returns the on-disk stub dir (for the definitions re-point;
/// caller owns cleanup) alongside the artifact. `None` = capture degraded;
/// the service ships without a surface and its pairs verdict unverifiable.
pub(crate) fn run_capture(
    sidecar: &TypeSidecar,
    repo_path: &str,
    service_id: &str,
    anchors: &[CaptureAnchor],
) -> Option<(PathBuf, CaptureStubArtifact)> {
    if anchors.is_empty() {
        return None;
    }
    let repo_root = std::path::Path::new(repo_path)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(repo_path));

    let unique = format!(
        "carrick-capture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let out_dir = std::env::temp_dir().join(unique);

    let result = match sidecar.capture_v2(
        &repo_root.to_string_lossy(),
        service_id,
        anchors,
        &out_dir.to_string_lossy(),
    ) {
        Ok(result) => result,
        Err(e) => {
            warn!("v2 capture failed for {}: {}", service_id, e);
            let _ = std::fs::remove_dir_all(&out_dir);
            return None;
        }
    };

    debug!(
        "v2 capture for {}: {} alias(es), usable_rate {:.3}, {} emitted file(s), bare_checkout={}",
        service_id,
        result.fidelity.total_aliases,
        result.fidelity.usable_rate,
        result.emitted_files.len(),
        result.bare_checkout
    );

    let stub_dir = PathBuf::from(&result.stub_dir);
    match CaptureStubArtifact::from_stub_dir(
        &stub_dir,
        &result.package_name,
        &result.ts_version,
        result.bare_checkout,
    ) {
        Ok(artifact) => Some((stub_dir, artifact)),
        Err(e) => {
            warn!("failed to read capture stub for {}: {}", service_id, e);
            let _ = std::fs::remove_dir_all(&stub_dir);
            None
        }
    }
}

// ===========================================================================
// Check time: pair building + check_v2 + outcome mapping
// ===========================================================================

/// Pseudo-method + join identity for a manifest entry, in exactly the format
/// `parse_producer_key` recovers from an edge's canonical producer key
/// (`("GET", "/orders/:id")`, `("SOCKET", "SERVER->CLIENT|event")`,
/// `("GRAPHQL", "query|field")`, `("PUBSUB", "topic")`).
fn join_identity(key: &OperationKey) -> Option<(String, String)> {
    match key {
        OperationKey::Http { method, path } => Some((method.to_uppercase(), path.clone())),
        OperationKey::Socket { event, direction } => Some((
            "SOCKET".to_string(),
            format!("{}|{}", direction.label(), event),
        )),
        OperationKey::Graphql { kind, field } => Some((
            "GRAPHQL".to_string(),
            format!("{}|{}", kind.as_str(), field),
        )),
        OperationKey::Pubsub { topic } => Some(("PUBSUB".to_string(), topic.clone())),
    }
}

/// Route-aware path normalization, ported from ts_check's `normalizePath`:
/// lowercase, single slashes, no trailing slash, every param syntax
/// (`:id`, `{id}`, `[id]`, `${expr}`) collapsed to `:param`.
fn normalize_match_path(input: &str) -> String {
    let mut normalized = input.to_lowercase();
    while normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    if !normalized.starts_with('/') {
        normalized.insert(0, '/');
    }
    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }
    normalized
        .split('/')
        .map(|seg| {
            let is_param = seg.starts_with(':')
                || (seg.starts_with('{') && seg.ends_with('}'))
                || (seg.starts_with('[') && seg.ends_with(']'))
                || seg.contains("${");
            if is_param {
                ":param".to_string()
            } else {
                seg.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Route-aware segment match, ported from ts_check's `pathsMatch`: equal
/// normalized paths, or segment-wise with `:param` as a wildcard.
fn paths_match(a: &str, b: &str) -> bool {
    let na = normalize_match_path(a);
    let nb = normalize_match_path(b);
    if na == nb {
        return true;
    }
    let sa: Vec<&str> = na.split('/').collect();
    let sb: Vec<&str> = nb.split('/').collect();
    if sa.len() != sb.len() {
        return false;
    }
    sa.iter()
        .zip(sb.iter())
        .all(|(x, y)| x == y || x.starts_with(':') || y.starts_with(':'))
}

/// Producer-specificity score, ported from ts_check's `calculateMatchScore`:
/// literal routes outrank parameterized ones for the same consumer.
fn match_score(producer_path: &str, consumer_path: &str) -> u8 {
    let np = normalize_match_path(producer_path);
    let nc = normalize_match_path(consumer_path);
    if np == nc {
        if producer_path == consumer_path {
            100
        } else {
            95
        }
    } else {
        90
    }
}

/// One built pair: the check spec plus everything the verdict join and the
/// findings projection need, so no information has to be re-parsed out of
/// labels after the verdict returns.
pub(crate) struct BuiltPair {
    pub spec: CheckPairSpec,
    /// Pseudo-method for the edge join (`GET` / `SOCKET` / `GRAPHQL` / `PUBSUB`).
    pub pseudo_method: String,
    /// Join identity: producer path (HTTP) or exact operation key tail.
    pub identity: String,
    /// Consumer call-site identity, `(file, line)` from the manifest entry.
    pub consumer_file: String,
    pub consumer_line: u32,
    pub type_kind: ManifestTypeKind,
    pub producer_alias: String,
    pub consumer_alias: String,
    pub producer_service: String,
    pub consumer_service: String,
    /// Set when the pair is unverifiable before any probe runs (a side's
    /// type_state is Unknown, or a side has no capture surface).
    pub pre_verdict: Option<(VerdictBucket, String)>,
}

struct ServiceEntry<'a> {
    service_id: &'a str,
    has_surface: bool,
    entry: &'a TypeManifestEntry,
}

/// Build check pairs from every participating repo's manifest, cross-service
/// only (same-identity pairs are dropped by every matcher, #397/#410).
///
/// Port of the ts_check manifest-matcher pairing semantics:
/// - HTTP: method + route-aware path match + type_kind, keeping only the
///   most specific producer(s) per consumer.
/// - socket/graphql/pubsub: exact operation-key match + type_kind.
///
/// An unresolved side (`type_state == Unknown`) or a side without a capture
/// surface produces a pair with a pre-set unverifiable verdict instead of a
/// probe (design: unresolved anchors generate no probe; a missing surface is
/// "peer scanned without a v2 surface — re-scan").
pub(crate) fn build_check_pairs(all_repo_data: &[CloudRepoData]) -> Vec<BuiltPair> {
    let mut producers: Vec<ServiceEntry> = Vec::new();
    let mut consumers: Vec<ServiceEntry> = Vec::new();

    for repo in all_repo_data {
        let service_id = repo
            .service_name
            .as_deref()
            .unwrap_or(repo.repo_name.as_str());
        let has_surface = repo
            .capture_stub
            .as_ref()
            .is_some_and(|s| s.artifact_version == CAPTURE_ARTIFACT_VERSION);
        let Some(entries) = repo.type_manifest.as_ref() else {
            continue;
        };
        for entry in entries {
            let target = match entry.role {
                ManifestRole::Producer => &mut producers,
                ManifestRole::Consumer => &mut consumers,
            };
            target.push(ServiceEntry {
                service_id,
                has_surface,
                entry,
            });
        }
    }

    let mut pairs: Vec<BuiltPair> = Vec::new();
    for consumer in &consumers {
        // Candidate producers, protocol-dispatched.
        let mut candidates: Vec<(&ServiceEntry, u8)> = Vec::new();
        for producer in &producers {
            if producer.service_id == consumer.service_id {
                continue;
            }
            if producer.entry.type_kind != consumer.entry.type_kind {
                continue;
            }
            match (&producer.entry.key, &consumer.entry.key) {
                (
                    OperationKey::Http {
                        method: pm,
                        path: pp,
                    },
                    OperationKey::Http {
                        method: cm,
                        path: cp,
                    },
                ) if pm.eq_ignore_ascii_case(cm) && paths_match(pp, cp) => {
                    candidates.push((producer, match_score(pp, cp)));
                }
                // Exact-key protocols: socket / graphql / pubsub.
                (
                    p @ (OperationKey::Socket { .. }
                    | OperationKey::Graphql { .. }
                    | OperationKey::Pubsub { .. }),
                    c,
                ) if p == c => {
                    candidates.push((producer, 100));
                }
                _ => {}
            }
        }
        if candidates.is_empty() {
            continue;
        }
        // HTTP specificity: keep only the best-scoring producer(s), mirroring
        // routing semantics (a literal route wins over :param).
        let best = candidates.iter().map(|(_, s)| *s).max().unwrap_or(0);
        for (producer, score) in candidates {
            if score != best {
                continue;
            }
            if let Some(pair) = build_pair(producer, consumer) {
                pairs.push(pair);
            }
        }
    }

    // Deterministic order regardless of repo download order.
    pairs.sort_by(|a, b| a.spec.pair_key.cmp(&b.spec.pair_key));
    pairs.dedup_by(|a, b| a.spec.pair_key == b.spec.pair_key);
    pairs
}

fn build_pair(producer: &ServiceEntry, consumer: &ServiceEntry) -> Option<BuiltPair> {
    let (pseudo_method, identity) = join_identity(&producer.entry.key)?;
    let protocol = match producer.entry.key {
        OperationKey::Http { .. } => ProbeProtocol::Http,
        OperationKey::Graphql { .. } => ProbeProtocol::Graphql,
        OperationKey::Socket { .. } => ProbeProtocol::Socket,
        OperationKey::Pubsub { .. } => ProbeProtocol::Pubsub,
    };
    // Socket/pubsub direction inverts regardless of manifest kind; the
    // sidecar's direction table keys on `both` for them.
    let type_kind = match (protocol, producer.entry.type_kind) {
        (ProbeProtocol::Socket | ProbeProtocol::Pubsub, _) => ProbeTypeKind::Both,
        (_, ManifestTypeKind::Request) => ProbeTypeKind::Request,
        (_, ManifestTypeKind::Response) => ProbeTypeKind::Response,
    };

    // Unique, deterministic pair key: both aliases embed the operation key,
    // role, kind, and (consumer-side) call site.
    let pair_key = format!(
        "{}/{}~{}/{}",
        producer.service_id,
        producer.entry.type_alias,
        consumer.service_id,
        consumer.entry.type_alias
    );

    let pre_verdict = if !producer.has_surface {
        Some((
            VerdictBucket::Unverifiable,
            format!(
                "producer service '{}' has no v2 type surface (older scan or capture degraded) — re-scan it",
                producer.service_id
            ),
        ))
    } else if !consumer.has_surface {
        Some((
            VerdictBucket::Unverifiable,
            format!(
                "consumer service '{}' has no v2 type surface (older scan or capture degraded) — re-scan it",
                consumer.service_id
            ),
        ))
    } else if producer.entry.type_state == ManifestTypeState::Unknown {
        Some((
            VerdictBucket::Unverifiable,
            "producer type was not resolved at capture time".to_string(),
        ))
    } else if consumer.entry.type_state == ManifestTypeState::Unknown {
        Some((
            VerdictBucket::Unverifiable,
            "consumer type was not resolved at capture time".to_string(),
        ))
    } else {
        None
    };

    Some(BuiltPair {
        spec: CheckPairSpec {
            pair_key,
            protocol,
            type_kind,
            producer: CheckPairEndpoint {
                service_name: producer.service_id.to_string(),
                alias: producer.entry.type_alias.clone(),
            },
            consumer: CheckPairEndpoint {
                service_name: consumer.service_id.to_string(),
                alias: consumer.entry.type_alias.clone(),
            },
        },
        pseudo_method,
        identity,
        consumer_file: consumer.entry.file_path.clone(),
        consumer_line: consumer.entry.line_number,
        type_kind: producer.entry.type_kind,
        producer_alias: producer.entry.type_alias.clone(),
        consumer_alias: consumer.entry.type_alias.clone(),
        producer_service: producer.service_id.to_string(),
        consumer_service: consumer.service_id.to_string(),
        pre_verdict,
    })
}

/// Materialize every artifact-carrying service's stub into `dest_root` and
/// return the check inputs. Stub dir names are keyed on the sanitized
/// service id (collisions suffixed) — same intent as `bundle_file_stems`.
pub(crate) fn materialize_stubs(
    all_repo_data: &[CloudRepoData],
    dest_root: &Path,
) -> Vec<CheckStubInput> {
    let mut used: HashMap<String, usize> = HashMap::new();
    let mut stubs = Vec::new();
    for repo in all_repo_data {
        let Some(artifact) = repo.capture_stub.as_ref() else {
            continue;
        };
        if artifact.artifact_version != CAPTURE_ARTIFACT_VERSION {
            continue;
        }
        let service_id = repo
            .service_name
            .as_deref()
            .unwrap_or(repo.repo_name.as_str());
        let base = service_id.replace(['/', '\\'], "_");
        let dir_name = match used.entry(base.clone()) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let n = e.get_mut();
                *n += 1;
                format!("{base}_{n}")
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(0);
                base
            }
        };
        let dest = dest_root.join(dir_name);
        if let Err(e) = artifact.materialize(&dest) {
            warn!("failed to materialize stub for {}: {}", service_id, e);
            continue;
        }
        stubs.push(CheckStubInput {
            service_name: service_id.to_string(),
            stub_dir: dest.to_string_lossy().into_owned(),
        });
    }
    stubs
}

/// Run the v2 check over the built pairs and produce the analyzer's pair
/// outcomes. Pairs with a pre-set verdict (no surface / unresolved side)
/// never reach the sidecar. When the check itself fails, every probing pair
/// degrades to unverifiable with the failure as the reason — never fatal to
/// the scan, and never read as compatible.
pub(crate) fn run_check(
    sidecar: &TypeSidecar,
    all_repo_data: &[CloudRepoData],
) -> Vec<PairCheckOutcome> {
    let pairs = build_check_pairs(all_repo_data);
    if pairs.is_empty() {
        return Vec::new();
    }

    let mut outcomes: Vec<PairCheckOutcome> = Vec::new();
    let mut probing: Vec<&BuiltPair> = Vec::new();
    for pair in &pairs {
        if let Some((bucket, reason)) = &pair.pre_verdict {
            outcomes.push(outcome_for(pair, *bucket, None, Some(reason.clone())));
        } else {
            probing.push(pair);
        }
    }

    if !probing.is_empty() {
        let unique = format!(
            "carrick-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let workspace_parent = std::env::temp_dir().join(unique);
        let stubs = materialize_stubs(all_repo_data, &workspace_parent);
        let specs: Vec<CheckPairSpec> = probing.iter().map(|p| p.spec.clone()).collect();

        let check_result = if stubs.is_empty() {
            Err(crate::services::type_sidecar::SidecarError::CheckFailed(
                "no capture stubs available".to_string(),
            ))
        } else {
            sidecar.check_v2(&stubs, &specs)
        };

        match check_result {
            Ok(result) => {
                let by_key: BTreeMap<&str, &crate::services::type_sidecar::CheckVerdict> = result
                    .verdicts
                    .iter()
                    .map(|v| (v.pair_key.as_str(), v))
                    .collect();
                for pair in &probing {
                    match by_key.get(pair.spec.pair_key.as_str()) {
                        Some(verdict) => outcomes.push(outcome_for(
                            pair,
                            verdict.bucket,
                            verdict.gate.clone(),
                            verdict.diagnostic.clone(),
                        )),
                        None => outcomes.push(outcome_for(
                            pair,
                            VerdictBucket::Unverifiable,
                            None,
                            Some("the check returned no verdict for this pair".to_string()),
                        )),
                    }
                }
                debug!(
                    "v2 check: {} pair(s) probed, {} pre-verdicted, ts {}",
                    probing.len(),
                    outcomes.len() - probing.len(),
                    result.ts_version
                );
            }
            Err(e) => {
                warn!(
                    "v2 check failed; all probing pairs degrade to unverifiable: {}",
                    e
                );
                let reason = format!("type check did not run: {}", e);
                for pair in &probing {
                    outcomes.push(outcome_for(
                        pair,
                        VerdictBucket::Unverifiable,
                        None,
                        Some(reason.clone()),
                    ));
                }
            }
        }

        let _ = std::fs::remove_dir_all(&workspace_parent);
    }

    // Deterministic order for every downstream consumer.
    outcomes.sort_by(|a, b| a.pair_key.cmp(&b.pair_key));
    outcomes
}

fn outcome_for(
    pair: &BuiltPair,
    bucket: VerdictBucket,
    gate: Option<String>,
    diagnostic: Option<String>,
) -> PairCheckOutcome {
    PairCheckOutcome {
        pair_key: pair.spec.pair_key.clone(),
        pseudo_method: pair.pseudo_method.clone(),
        identity: pair.identity.clone(),
        consumer_file: pair.consumer_file.clone(),
        consumer_line: pair.consumer_line,
        type_kind: pair.type_kind,
        bucket,
        gate,
        diagnostic,
        producer_alias: pair.producer_alias.clone(),
        consumer_alias: pair.consumer_alias.clone(),
        producer_service: pair.producer_service.clone(),
        consumer_service: pair.consumer_service.clone(),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_storage::TypeEvidence;
    use crate::services::type_sidecar::InferKind;
    use crate::type_manifest::{build_manifest_type_alias, build_manifest_type_alias_with_call_id};

    fn entry(
        key: OperationKey,
        role: ManifestRole,
        type_kind: ManifestTypeKind,
        type_alias: &str,
        file_path: &str,
        line_number: u32,
        type_state: ManifestTypeState,
    ) -> TypeManifestEntry {
        TypeManifestEntry {
            key,
            role,
            type_kind,
            type_alias: type_alias.to_string(),
            file_path: file_path.to_string(),
            line_number,
            is_explicit: type_state == ManifestTypeState::Explicit,
            type_state,
            evidence: TypeEvidence {
                file_path: file_path.to_string(),
                span_start: None,
                span_end: None,
                line_number,
                infer_kind: InferKind::ResponseBody,
                is_explicit: false,
                type_state,
            },
            resolved_definition: None,
            expanded_definition: None,
            primary_type_symbol: None,
        }
    }

    fn repo(
        repo_name: &str,
        service_name: Option<&str>,
        manifest: Vec<TypeManifestEntry>,
        capture_stub: Option<CaptureStubArtifact>,
    ) -> CloudRepoData {
        CloudRepoData {
            repo_name: repo_name.to_string(),
            service_name: service_name.map(str::to_string),
            endpoints: Vec::new(),
            calls: Vec::new(),
            mounts: Vec::new(),
            apps: HashMap::new(),
            imported_handlers: Vec::new(),
            function_definitions: HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: if manifest.is_empty() {
                None
            } else {
                Some(manifest)
            },
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            cached_extraction_config: None,
            package_json_hash: None,
            cache_version: None,
            type_extraction_status: None,
            compat_verdicts: None,
            capture_stub,
        }
    }

    fn fake_artifact() -> CaptureStubArtifact {
        CaptureStubArtifact {
            artifact_version: CAPTURE_ARTIFACT_VERSION,
            package_name: "@carrick/test".to_string(),
            ts_version: "5.8.3".to_string(),
            bare_checkout: true,
            files: BTreeMap::new(),
        }
    }

    // ---- derive_capture_anchors -------------------------------------------

    /// Precedence per alias mirrors the v1 bundle: symbol wins over infer,
    /// infer wins over literal; aliases pass through byte-identical.
    #[test]
    fn derive_anchors_precedence_and_alias_passthrough() {
        let explicit = vec![SymbolRequest {
            symbol_name: "Order".to_string(),
            source_file: "src/types.ts".to_string(),
            alias: Some("Endpoint_a_Response".to_string()),
            array_depth: Some(1),
        }];
        let infer = vec![
            crate::services::type_sidecar::InferRequestItem {
                file_path: "/repo/src/handler.ts".to_string(),
                line_number: 7,
                span_start: Some(10),
                span_end: Some(20),
                expression_text: None,
                expression_line: None,
                infer_kind: InferKind::ResponseBody,
                alias: Some("Endpoint_a_Response".to_string()),
                param_name: None,
            },
            crate::services::type_sidecar::InferRequestItem {
                file_path: "/repo/src/handler.ts".to_string(),
                line_number: 9,
                span_start: None,
                span_end: None,
                expression_text: Some("payload".to_string()),
                expression_line: Some(9),
                infer_kind: InferKind::ResponseBody,
                alias: Some("Endpoint_b_Response".to_string()),
                param_name: None,
            },
        ];
        let inline = vec![
            ("Endpoint_b_Response".to_string(), "Widget".to_string()),
            (
                "Endpoint_c_Response".to_string(),
                "{ ok: boolean }".to_string(),
            ),
        ];

        let anchors = derive_capture_anchors(&explicit, &infer, &inline, &[], "/repo");
        assert_eq!(anchors.len(), 3);

        match &anchors[0] {
            CaptureAnchor::Symbol {
                alias,
                array_depth,
                source_file,
                ..
            } => {
                assert_eq!(alias, "Endpoint_a_Response");
                assert_eq!(*array_depth, Some(1));
                assert_eq!(source_file, "src/types.ts");
            }
            other => panic!("expected symbol anchor, got {:?}", other),
        }
        match &anchors[1] {
            CaptureAnchor::Infer {
                alias, source_file, ..
            } => {
                assert_eq!(alias, "Endpoint_b_Response");
                // Absolute path under the repo root is relativized for the wire.
                assert_eq!(source_file, "src/handler.ts");
            }
            other => panic!("expected infer anchor, got {:?}", other),
        }
        match &anchors[2] {
            CaptureAnchor::Literal {
                alias, type_text, ..
            } => {
                assert_eq!(alias, "Endpoint_c_Response");
                assert_eq!(type_text, "{ ok: boolean }");
            }
            other => panic!("expected literal anchor, got {:?}", other),
        }
    }

    // ---- contains_disqualifying_top_type ----------------------------------

    /// The shared notion: any/unknown as a TYPE token at any position
    /// disqualifies (adversarial-review finding 2); property names, string
    /// literals, and ordinary identifiers containing the words do not.
    #[test]
    fn disqualifying_top_type_token_scan() {
        for text in [
            "any",
            "unknown",
            "any[]",
            "Array<any>",
            "Promise<any>",
            "Record<string, any>",
            "{ [k: string]: any }",
            "{ orderId: string; metadata: any }",
            "{ a: { b: unknown } }",
            "{ items: any[] }",
            "Promise<{ data: unknown }>",
            "(string | any)[]",
        ] {
            assert!(
                contains_disqualifying_top_type(text),
                "must disqualify: {text}"
            );
        }
        for text in [
            "{ ok: boolean }",
            "string[]",
            "Promise<{ a: string }>",
            "{ kind: \"any\" }",
            "{ kind: 'unknown' }",
            "{ any: string }",
            "{ any?: string }",
            "{ unknown: number }",
            "{ company: string }",
            "Anything",
            "{ unknownField: number; anyhow: string }",
        ] {
            assert!(
                !contains_disqualifying_top_type(text),
                "must NOT disqualify: {text}"
            );
        }
    }

    /// Container-decayed v1 inference text (`Promise<any>`, `any[]`, member
    /// any) must NOT ride a literal anchor: pre-fix it became a literal
    /// surface alias that probed clean and read compatible. Rejected text
    /// falls back to the locator-based infer anchor.
    #[test]
    fn derive_anchors_rejects_container_decayed_inferred_text() {
        let infer_item = |alias: &str| crate::services::type_sidecar::InferRequestItem {
            file_path: "src/bus.ts".to_string(),
            line_number: 4,
            span_start: None,
            span_end: None,
            expression_text: Some("payload".to_string()),
            expression_line: Some(4),
            infer_kind: InferKind::Expression,
            alias: Some(alias.to_string()),
            param_name: None,
        };
        let inferred_type = |alias: &str, text: &str| crate::services::type_sidecar::InferredType {
            alias: alias.to_string(),
            type_string: text.to_string(),
            is_explicit: false,
            source_location: crate::services::type_sidecar::SourceLocation {
                file_path: "src/bus.ts".to_string(),
                start_line: 4,
                end_line: 4,
                start_column: None,
                end_column: None,
            },
            infer_kind: InferKind::Expression,
            primary_type_symbol: None,
            array_depth: None,
        };

        let infer = vec![
            infer_item("Pub_PromiseAny"),
            infer_item("Pub_ArrayAny"),
            infer_item("Pub_MemberAny"),
            infer_item("Pub_Clean"),
        ];
        let inferred = vec![
            inferred_type("Pub_PromiseAny", "Promise<any>"),
            inferred_type("Pub_ArrayAny", "any[]"),
            inferred_type("Pub_MemberAny", "{ orderId: string; metadata: any }"),
            inferred_type("Pub_Clean", "{ ok: boolean }"),
        ];

        let anchors = derive_capture_anchors(&[], &infer, &[], &inferred, ".");
        assert_eq!(anchors.len(), 4);
        for (anchor, alias) in
            anchors
                .iter()
                .zip(["Pub_PromiseAny", "Pub_ArrayAny", "Pub_MemberAny"])
        {
            match anchor {
                CaptureAnchor::Infer { alias: a, .. } => assert_eq!(a, alias),
                other => {
                    panic!("{alias}: decayed text must fall back to an infer anchor, got {other:?}")
                }
            }
        }
        match &anchors[3] {
            CaptureAnchor::Literal {
                alias, type_text, ..
            } => {
                assert_eq!(alias, "Pub_Clean");
                assert_eq!(type_text, "{ ok: boolean }");
            }
            other => panic!("clean text must stay a literal anchor, got {other:?}"),
        }
    }

    /// An infer-request alias whose v1 inference produced a real shape rides
    /// a LITERAL anchor carrying that text (the kind-aware inference result);
    /// a placeholder text (`unknown`) keeps the locator-based infer anchor.
    #[test]
    fn derive_anchors_prefers_v1_inferred_text_for_infer_aliases() {
        let infer_item = |alias: &str| crate::services::type_sidecar::InferRequestItem {
            file_path: "src/bus.ts".to_string(),
            line_number: 4,
            span_start: None,
            span_end: None,
            expression_text: Some("payload".to_string()),
            expression_line: Some(4),
            infer_kind: InferKind::Expression,
            alias: Some(alias.to_string()),
            param_name: None,
        };
        let inferred_type = |alias: &str, text: &str| crate::services::type_sidecar::InferredType {
            alias: alias.to_string(),
            type_string: text.to_string(),
            is_explicit: false,
            source_location: crate::services::type_sidecar::SourceLocation {
                file_path: "src/bus.ts".to_string(),
                start_line: 4,
                end_line: 4,
                start_column: None,
                end_column: None,
            },
            infer_kind: InferKind::Expression,
            primary_type_symbol: None,
            array_depth: None,
        };

        let infer = vec![infer_item("Pub_Resolved"), infer_item("Pub_Unresolved")];
        let inferred = vec![
            inferred_type("Pub_Resolved", "{ time: string; item: string; }"),
            inferred_type("Pub_Unresolved", "unknown"),
        ];

        let anchors = derive_capture_anchors(&[], &infer, &[], &inferred, ".");
        assert_eq!(anchors.len(), 2);
        match &anchors[0] {
            CaptureAnchor::Literal {
                alias,
                type_text,
                anchor_origin,
            } => {
                assert_eq!(alias, "Pub_Resolved");
                assert_eq!(type_text, "{ time: string; item: string; }");
                assert_eq!(*anchor_origin, AnchorOrigin::DeterministicInfer);
            }
            other => panic!(
                "expected literal anchor from inferred text, got {:?}",
                other
            ),
        }
        match &anchors[1] {
            CaptureAnchor::Infer { alias, .. } => assert_eq!(alias, "Pub_Unresolved"),
            other => panic!("expected locator infer anchor, got {:?}", other),
        }
    }

    // ---- build_check_pairs ------------------------------------------------

    /// HTTP pairing is method + route-aware path + type_kind, cross-service
    /// only, and a literal producer outranks a parameterized one for the same
    /// consumer (the ts_check specificity rule).
    #[test]
    fn build_pairs_http_specificity_and_cross_service_only() {
        let key_param = OperationKey::http("GET", "/users/:id");
        let key_literal = OperationKey::http("GET", "/users/me");
        let consumer_key = OperationKey::http("GET", "/users/me");

        let producer_repo = repo(
            "api",
            None,
            vec![
                entry(
                    key_param.clone(),
                    ManifestRole::Producer,
                    ManifestTypeKind::Response,
                    "P_param",
                    "src/routes.ts",
                    3,
                    ManifestTypeState::Explicit,
                ),
                entry(
                    key_literal.clone(),
                    ManifestRole::Producer,
                    ManifestTypeKind::Response,
                    "P_literal",
                    "src/routes.ts",
                    9,
                    ManifestTypeState::Explicit,
                ),
            ],
            Some(fake_artifact()),
        );
        let consumer_repo = repo(
            "web",
            None,
            vec![entry(
                consumer_key.clone(),
                ManifestRole::Consumer,
                ManifestTypeKind::Response,
                "C_me",
                "src/client.ts",
                12,
                ManifestTypeState::Explicit,
            )],
            Some(fake_artifact()),
        );
        // A same-identity repo carrying both sides must produce no pair.
        let self_repo = repo(
            "web",
            None,
            vec![entry(
                key_literal.clone(),
                ManifestRole::Producer,
                ManifestTypeKind::Response,
                "P_self",
                "src/self.ts",
                1,
                ManifestTypeState::Explicit,
            )],
            Some(fake_artifact()),
        );

        let pairs = build_check_pairs(&[producer_repo, consumer_repo, self_repo]);
        assert_eq!(pairs.len(), 1, "one pair: the most specific producer wins");
        let pair = &pairs[0];
        assert_eq!(pair.producer_alias, "P_literal");
        assert_eq!(pair.consumer_alias, "C_me");
        assert_eq!(pair.pseudo_method, "GET");
        assert_eq!(pair.identity, "/users/me");
        assert_eq!(pair.consumer_file, "src/client.ts");
        assert_eq!(pair.consumer_line, 12);
        assert!(pair.pre_verdict.is_none());
        assert_eq!(pair.spec.protocol, ProbeProtocol::Http);
        assert_eq!(pair.spec.type_kind, ProbeTypeKind::Response);
    }

    /// Exact-key protocols pair on equal operation keys; socket/pubsub pairs
    /// probe as `both` (the direction table inverts them), and the identity
    /// matches what `parse_producer_key` recovers from an edge.
    #[test]
    fn build_pairs_exact_key_protocols() {
        let topic = OperationKey::pubsub("order.placed");
        let producer_repo = repo(
            "worker",
            Some("billing"),
            vec![entry(
                topic.clone(),
                ManifestRole::Producer,
                ManifestTypeKind::Response,
                "P_topic",
                "src/subscriber.ts",
                4,
                ManifestTypeState::Explicit,
            )],
            Some(fake_artifact()),
        );
        let consumer_repo = repo(
            "orders",
            Some("orders-engine"),
            vec![entry(
                topic.clone(),
                ManifestRole::Consumer,
                ManifestTypeKind::Response,
                "C_topic",
                "src/publisher.ts",
                21,
                ManifestTypeState::Explicit,
            )],
            Some(fake_artifact()),
        );

        let pairs = build_check_pairs(&[producer_repo, consumer_repo]);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].pseudo_method, "PUBSUB");
        assert_eq!(pairs[0].identity, "order.placed");
        assert_eq!(pairs[0].spec.protocol, ProbeProtocol::Pubsub);
        assert_eq!(pairs[0].spec.type_kind, ProbeTypeKind::Both);
        assert_eq!(pairs[0].producer_service, "billing");
        assert_eq!(pairs[0].consumer_service, "orders-engine");
    }

    /// An unresolved side or a missing capture surface pre-verdicts the pair
    /// unverifiable — it never reaches a probe, and the reason travels.
    #[test]
    fn build_pairs_pre_verdicts_unresolved_and_surfaceless() {
        let key = OperationKey::http("GET", "/orders");
        let no_surface_producer = repo(
            "api",
            None,
            vec![entry(
                key.clone(),
                ManifestRole::Producer,
                ManifestTypeKind::Response,
                "P",
                "src/routes.ts",
                3,
                ManifestTypeState::Explicit,
            )],
            None, // no capture stub: older scan or degraded capture
        );
        let consumer_ok = repo(
            "web",
            None,
            vec![entry(
                key.clone(),
                ManifestRole::Consumer,
                ManifestTypeKind::Response,
                "C",
                "src/client.ts",
                8,
                ManifestTypeState::Explicit,
            )],
            Some(fake_artifact()),
        );
        let pairs = build_check_pairs(&[no_surface_producer, consumer_ok.clone()]);
        assert_eq!(pairs.len(), 1);
        let (bucket, reason) = pairs[0].pre_verdict.as_ref().expect("pre-verdict");
        assert_eq!(*bucket, VerdictBucket::Unverifiable);
        assert!(reason.contains("no v2 type surface"), "{reason}");

        // Unknown type_state on a side with a surface: unresolved anchors
        // generate no probe (design, Check step 9).
        let unresolved_producer = repo(
            "api",
            None,
            vec![entry(
                key.clone(),
                ManifestRole::Producer,
                ManifestTypeKind::Response,
                "P",
                "src/routes.ts",
                3,
                ManifestTypeState::Unknown,
            )],
            Some(fake_artifact()),
        );
        let pairs = build_check_pairs(&[unresolved_producer, consumer_ok]);
        assert_eq!(pairs.len(), 1);
        let (bucket, reason) = pairs[0].pre_verdict.as_ref().expect("pre-verdict");
        assert_eq!(*bucket, VerdictBucket::Unverifiable);
        assert!(reason.contains("not resolved at capture time"), "{reason}");
    }

    // ---- end-to-end: capture_v2 -> check_v2 -> pair outcomes -> edge join --

    /// Full deterministic integration of the WP3 path against the corpus-2
    /// fixture pair (orders-engine's OrderPlaced.total is a Money object;
    /// billing-svc expects a bare number — the deliberate incompatibility):
    ///
    ///   1. capture_v2 both services through the Rust client,
    ///   2. artifacts ride CloudRepoData.capture_stub,
    ///   3. run_check builds the pair, materializes stubs, runs check_v2,
    ///   4. the incompatible verdict joins the CrossRepoMatch edge by
    ///      structured identity (pair-ID join, no label parsing), and
    ///   5. outcomes are stable across two independent check runs.
    ///
    /// The check path is deterministic given the anchors (no LLM anywhere);
    /// the pnpm install is local-only for these bare fixtures.
    #[test]
    fn corpus_capture_check_join_end_to_end() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let sidecar_path = manifest_dir.join("src/sidecar/dist/src/index.js");
        if !sidecar_path.exists() {
            eprintln!("Skipping test: sidecar not built (cd src/sidecar && npm run build)");
            return;
        }
        let orders_repo = manifest_dir.join("tests/fixtures/xrepo-corpus-2/orders-engine");
        let billing_repo = manifest_dir.join("tests/fixtures/xrepo-corpus-2/billing-svc");
        assert!(orders_repo.exists(), "fixture missing: {orders_repo:?}");
        assert!(billing_repo.exists(), "fixture missing: {billing_repo:?}");

        let sidecar = TypeSidecar::spawn(&sidecar_path).expect("spawn sidecar");
        sidecar.start_init(&orders_repo, None);
        sidecar
            .wait_ready(std::time::Duration::from_secs(60))
            .expect("sidecar init");

        // Aliases exactly as the manifest builder derives them.
        let key = OperationKey::http("GET", "/orders/latest");
        let producer_alias =
            build_manifest_type_alias(&key, ManifestRole::Producer, ManifestTypeKind::Response);
        let consumer_call_id = crate::type_manifest::build_call_site_id(
            "src/billing-call.ts",
            5,
            &key,
            billing_repo.to_str().unwrap(),
        );
        let consumer_alias = build_manifest_type_alias_with_call_id(
            &key,
            ManifestRole::Consumer,
            ManifestTypeKind::Response,
            Some(&consumer_call_id),
        );

        // Capture both services (symbol anchors on each side's OrderPlaced).
        let (orders_stub, orders_artifact) = run_capture(
            &sidecar,
            orders_repo.to_str().unwrap(),
            "orders-engine",
            &[CaptureAnchor::Symbol {
                alias: producer_alias.clone(),
                symbol_name: "OrderPlaced".to_string(),
                source_file: "src/types/order.ts".to_string(),
                anchor_origin: AnchorOrigin::LlmSymbol,
                array_depth: None,
            }],
        )
        .expect("orders-engine capture");
        let (billing_stub, billing_artifact) = run_capture(
            &sidecar,
            billing_repo.to_str().unwrap(),
            "billing-svc",
            &[CaptureAnchor::Symbol {
                alias: consumer_alias.clone(),
                symbol_name: "OrderPlaced".to_string(),
                source_file: "src/types/billing.ts".to_string(),
                anchor_origin: AnchorOrigin::LlmSymbol,
                array_depth: None,
            }],
        )
        .expect("billing-svc capture");
        let _ = std::fs::remove_dir_all(&orders_stub);
        let _ = std::fs::remove_dir_all(&billing_stub);

        let all_repo_data = vec![
            repo(
                "orders-engine",
                None,
                vec![entry(
                    key.clone(),
                    ManifestRole::Producer,
                    ManifestTypeKind::Response,
                    &producer_alias,
                    "src/routes.ts",
                    3,
                    ManifestTypeState::Explicit,
                )],
                Some(orders_artifact),
            ),
            repo(
                "billing-svc",
                None,
                vec![entry(
                    key.clone(),
                    ManifestRole::Consumer,
                    ManifestTypeKind::Response,
                    &consumer_alias,
                    "src/billing-call.ts",
                    5,
                    ManifestTypeState::Explicit,
                )],
                Some(billing_artifact),
            ),
        ];

        let outcomes = run_check(&sidecar, &all_repo_data);
        assert_eq!(outcomes.len(), 1, "exactly one matched pair");
        let outcome = &outcomes[0];
        assert_eq!(
            outcome.bucket,
            VerdictBucket::Incompatible,
            "the corpus-2 Money-object vs number mismatch must verdict \
             incompatible; diagnostic: {:?}",
            outcome.diagnostic
        );
        let diagnostic = outcome.diagnostic.as_deref().unwrap_or("");
        assert!(
            !diagnostic.contains("/tmp/") && !diagnostic.contains("/private/"),
            "diagnostic leaked a scratch path: {diagnostic}"
        );

        // Pair-ID join onto the CrossRepoMatch edge — structured identity,
        // no label parsing anywhere.
        let mut edges = vec![crate::analyzer::CrossRepoMatch {
            producer_repo: "orders-engine".to_string(),
            producer_key: key.canonical(),
            consumer_repo: "billing-svc".to_string(),
            consumer_key: key.canonical(),
            consumer_location: Some("src/billing-call.ts:5:9".to_string()),
            match_score: 1.0,
            type_compatible: None,
            mismatch_reason: None,
            producer_provenance: Default::default(),
            relationship: carrick_match::MatchRelationship::ProducerConsumer,
        }];
        crate::analyzer::apply_pair_outcomes(&outcomes, &mut edges);
        assert_eq!(
            edges[0].type_compatible,
            Some(false),
            "the incompatible verdict must land on the edge"
        );
        assert!(edges[0].mismatch_reason.is_some());

        // Determinism: a second independent check run yields the same
        // outcomes (pair keys, buckets, diagnostics).
        let outcomes_again = run_check(&sidecar, &all_repo_data);
        let flat = |v: &[PairCheckOutcome]| {
            v.iter()
                .map(|o| {
                    format!(
                        "{}|{:?}|{}",
                        o.pair_key,
                        o.bucket,
                        o.diagnostic.as_deref().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            flat(&outcomes),
            flat(&outcomes_again),
            "check outcomes must be byte-stable across runs"
        );
    }
}
