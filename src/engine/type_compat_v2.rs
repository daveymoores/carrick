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

/// Derive one capture anchor per alias from the collected v1 type requests.
///
/// Precedence mirrors the v1 bundle: an explicit symbol request wins over an
/// infer request for the same alias, which wins over an inline literal. The
/// alias strings are consumed as-is — this function never re-derives or
/// rewrites an alias, so the manifest join keys stay byte-identical.
///
/// `anchor_origin` mapping is pragmatic for WP3: symbol/literal anchors are
/// LLM-sourced (`llm-symbol`), infer anchors are locator-driven
/// (`deterministic-infer`). Refining backfill attribution is follow-up work.
pub(crate) fn derive_capture_anchors(
    explicit: &[SymbolRequest],
    infer: &[InferRequestItem],
    inline_aliases: &[(String, String)],
    repo_root: &str,
) -> Vec<CaptureAnchor> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut anchors: Vec<CaptureAnchor> = Vec::new();

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
                ) => {
                    if pm.eq_ignore_ascii_case(cm) && paths_match(pp, cp) {
                        candidates.push((producer, match_score(pp, cp)));
                    }
                }
                (p, c) if p == c => {
                    // Exact-key protocols: socket / graphql / pubsub.
                    if !matches!(p, OperationKey::Http { .. }) {
                        candidates.push((producer, 100));
                    }
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
