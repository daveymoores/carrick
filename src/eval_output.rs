//! Machine-readable scanner output for the eval harness.
//!
//! When `CARRICK_OUTPUT_JSON` is set, the engine emits an [`EvalProjection`]
//! instead of the human-readable Markdown report. This is a *dedicated* shape,
//! deliberately decoupled from the internal analyzer types, so the eval scoring
//! contract stays stable across refactors of `ApiEndpointDetails` and friends.
//! The cross-repo eval (slice S1) scores endpoint/call set accuracy, cross-repo
//! producer→consumer matches, per-op type resolution, and dependency conflicts
//! from this projection.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::analyzer::{
    ApiAnalysisResult, ApiEndpointDetails, ConflictSeverity,
    CrossRepoMatch as AnalyzerCrossRepoMatch, DependencyConflict,
};
use crate::cloud_storage::TypeManifestEntry;
use crate::operation::OperationKey;

/// The full eval projection of a single scan: the producer endpoints, the
/// consumer calls, the cross-repo edges between them, and the dependency
/// conflicts the scanner extracted.
#[derive(Debug, Serialize, Deserialize)]
pub struct EvalProjection {
    pub endpoints: Vec<EvalOp>,
    pub calls: Vec<EvalOp>,
    /// Structured producer→consumer edges (contract §2). The scorer keys
    /// cross-repo match P/R/F1 and the compat-verdict metric off these.
    pub cross_repo_matches: Vec<CrossRepoMatch>,
    /// Dependency version conflicts across the scanned repos (contract §4).
    pub dependency_conflicts: Vec<EvalDependencyConflict>,
}

/// One extracted operation (endpoint or call), flattened for scoring.
#[derive(Debug, Serialize, Deserialize)]
pub struct EvalOp {
    /// Stable identity from `OperationKey::canonical()`, e.g. `http|GET|/users`.
    pub key: String,
    /// `"http" | "graphql" | "socket"`.
    pub protocol: String,
    /// HTTP method, when this is an HTTP operation.
    pub method: Option<String>,
    /// HTTP path, GraphQL field, or socket event.
    pub path: Option<String>,
    pub handler: Option<String>,
    pub request_type: Option<String>,
    pub response_type: Option<String>,
    pub file: String,
    pub line: u32,
    // --- Type-manifest fields (contract §3), joined by OperationKey ---
    /// The type alias used in the bundled `.d.ts` file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_alias: Option<String>,
    /// `ManifestTypeState` in String form: `"Explicit"` / `"Implicit"` / `"Unknown"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_state: Option<String>,
    /// Original declaration text as written (sidecar `resolved_definition`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_definition: Option<String>,
    /// Compiler-expanded form with all types inlined (sidecar `expanded_definition`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded_definition: Option<String>,
    /// Whether the type was explicitly annotated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_explicit: Option<bool>,
    /// The LLM-emitted type anchor (contract §3, §5 row 5).
    ///
    /// The real anchor (`primary_type_symbol` on the file-analyzer result) is
    /// threaded onto the type manifest at build time, joined by `(file_path,
    /// line_number)`, so this carries the real source symbol (`StatusResponse`)
    /// rather than the hashed `type_alias`. Falls back to `type_alias` only when
    /// no real anchor was extracted for the op.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_type_symbol: Option<String>,
}

/// A structured producer→consumer edge (contract §2 FROZEN field set).
#[derive(Debug, Serialize, Deserialize)]
pub struct CrossRepoMatch {
    pub producer_repo: String,
    pub producer_key: String,
    pub consumer_repo: String,
    pub consumer_key: String,
    pub match_score: f64,
    /// `None` = compat NOT evaluated for this edge; `Some(b)` = evaluated.
    /// Omitted from JSON when `None` so the scorer never reads absent compat
    /// data as a verdict (the `ts_check_dir` trap, contract §7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_compatible: Option<bool>,
    /// `Some(..)` iff `type_compatible == Some(false)`; omitted otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mismatch_reason: Option<String>,
}

/// A dependency version conflict (contract §4). `versions` is sorted ascending
/// for stable comparison against the answer key.
#[derive(Debug, Serialize, Deserialize)]
pub struct EvalDependencyConflict {
    pub package: String,
    pub versions: Vec<String>,
    pub severity: String,
}

impl EvalProjection {
    /// Build the projection from analyzer results plus the merged type manifest.
    /// The manifest is joined per op by `OperationKey`; deps come straight from
    /// `issues.dependency_conflicts`; matches from `result.cross_repo_matches`.
    pub fn from_results(result: &ApiAnalysisResult, type_manifest: &[TypeManifestEntry]) -> Self {
        let manifest_index = ManifestIndex::build(type_manifest);
        // Canonical ordering so the projection is deterministic across runs:
        // the matcher's edge order and analyze_dependencies' conflict order are
        // both unstable, which would otherwise flake the cassette hard gate.
        let mut cross_repo_matches: Vec<CrossRepoMatch> = result
            .cross_repo_matches
            .iter()
            .map(CrossRepoMatch::from_analyzer)
            .collect();
        cross_repo_matches.sort_by(|a, b| {
            (
                &a.producer_repo,
                &a.producer_key,
                &a.consumer_repo,
                &a.consumer_key,
            )
                .cmp(&(
                    &b.producer_repo,
                    &b.producer_key,
                    &b.consumer_repo,
                    &b.consumer_key,
                ))
        });
        let mut dependency_conflicts: Vec<EvalDependencyConflict> = result
            .issues
            .dependency_conflicts
            .iter()
            .map(EvalDependencyConflict::from_conflict)
            .collect();
        dependency_conflicts.sort_by(|a, b| a.package.cmp(&b.package));
        Self {
            endpoints: result
                .endpoints
                .iter()
                .map(|d| EvalOp::from_details(d, &manifest_index))
                .collect(),
            calls: result
                .calls
                .iter()
                .map(|d| EvalOp::from_details(d, &manifest_index))
                .collect(),
            cross_repo_matches,
            dependency_conflicts,
        }
    }
}

/// The manifest fields an op carries, collapsed from the request/response
/// manifest entries for a single operation key.
#[derive(Default)]
struct ManifestFields {
    type_alias: Option<String>,
    type_state: Option<String>,
    resolved_definition: Option<String>,
    expanded_definition: Option<String>,
    is_explicit: Option<bool>,
    /// Real LLM type-anchor symbol, threaded onto the manifest entry at build
    /// time. Distinct from `type_alias` (the synthetic hashed
    /// `Endpoint_<hash>_Response` name); the anchor metric scores against this.
    primary_type_symbol: Option<String>,
}

/// Lookup from canonical operation key → manifest fields. The manifest carries
/// up to two entries per op (request + response); we prefer the entry that has
/// resolved-type detail so the projection surfaces the richest available type.
struct ManifestIndex {
    by_key: HashMap<String, ManifestFields>,
}

impl ManifestIndex {
    fn build(entries: &[TypeManifestEntry]) -> Self {
        let mut by_key: HashMap<String, ManifestFields> = HashMap::new();
        for entry in entries {
            let key = entry.key.canonical();
            let slot = by_key.entry(key).or_default();
            // Prefer an entry that resolved a concrete definition; otherwise
            // keep the first-seen alias/state so the field is at least present.
            let entry_has_definition =
                entry.resolved_definition.is_some() || entry.expanded_definition.is_some();
            let slot_has_definition =
                slot.resolved_definition.is_some() || slot.expanded_definition.is_some();
            if slot.type_alias.is_none() || (entry_has_definition && !slot_has_definition) {
                slot.type_alias = Some(entry.type_alias.clone());
                slot.type_state = Some(manifest_type_state_string(entry.type_state));
                slot.resolved_definition = entry.resolved_definition.clone();
                slot.expanded_definition = entry.expanded_definition.clone();
                slot.is_explicit = Some(entry.is_explicit);
            }
            // The anchor symbol is independent of the definition-richness race
            // above: keep the first non-None symbol seen for this op so a
            // response entry that wins on definition but carries no symbol does
            // not erase a request entry's symbol.
            if slot.primary_type_symbol.is_none() {
                slot.primary_type_symbol = entry.primary_type_symbol.clone();
            }
        }
        Self { by_key }
    }

    fn get(&self, key: &str) -> Option<&ManifestFields> {
        self.by_key.get(key)
    }
}

/// `ManifestTypeState` → its String form, matching the manifest's serde naming
/// (`"Explicit"` / `"Implicit"` / `"Unknown"`).
fn manifest_type_state_string(state: crate::cloud_storage::ManifestTypeState) -> String {
    use crate::cloud_storage::ManifestTypeState::*;
    match state {
        Explicit => "Explicit",
        Implicit => "Implicit",
        Unknown => "Unknown",
    }
    .to_string()
}

impl EvalOp {
    fn from_details(d: &ApiEndpointDetails, manifest: &ManifestIndex) -> Self {
        let (protocol, method, path) = project_key(&d.key);
        let (file, line) = split_location(&d.file_path);
        let canonical = d.key.canonical();
        let fields = manifest.get(&canonical);
        EvalOp {
            key: canonical,
            protocol,
            method,
            path,
            handler: d.handler_name.clone(),
            request_type: d
                .request_type
                .as_ref()
                .map(|t| t.composite_type_string.clone()),
            response_type: d
                .response_type
                .as_ref()
                .map(|t| t.composite_type_string.clone()),
            file,
            line,
            type_alias: fields.and_then(|f| f.type_alias.clone()),
            type_state: fields.and_then(|f| f.type_state.clone()),
            resolved_definition: fields.and_then(|f| f.resolved_definition.clone()),
            expanded_definition: fields.and_then(|f| f.expanded_definition.clone()),
            is_explicit: fields.and_then(|f| f.is_explicit),
            // The real LLM type anchor (#233): the symbol threaded onto the
            // manifest entry, falling back to the hashed `type_alias` only when
            // no real symbol was extracted for this op.
            primary_type_symbol: fields.and_then(|f| {
                f.primary_type_symbol
                    .clone()
                    .or_else(|| f.type_alias.clone())
            }),
        }
    }
}

impl CrossRepoMatch {
    fn from_analyzer(m: &AnalyzerCrossRepoMatch) -> Self {
        CrossRepoMatch {
            producer_repo: m.producer_repo.clone(),
            producer_key: m.producer_key.clone(),
            consumer_repo: m.consumer_repo.clone(),
            consumer_key: m.consumer_key.clone(),
            match_score: m.match_score,
            type_compatible: m.type_compatible,
            mismatch_reason: m.mismatch_reason.clone(),
        }
    }
}

impl EvalDependencyConflict {
    fn from_conflict(c: &DependencyConflict) -> Self {
        let mut versions: Vec<String> = c.repos.iter().map(|r| r.version.clone()).collect();
        versions.sort();
        versions.dedup();
        EvalDependencyConflict {
            package: c.package_name.clone(),
            versions,
            severity: conflict_severity_string(&c.severity),
        }
    }
}

/// Render `ConflictSeverity` to a stable lowercase string. Mirrors the enum's
/// variant identity (`critical` / `warning` / `info`) so the eval answer key
/// compares against a stable token rather than display prose.
fn conflict_severity_string(severity: &ConflictSeverity) -> String {
    match severity {
        ConflictSeverity::Critical => "critical",
        ConflictSeverity::Warning => "warning",
        ConflictSeverity::Info => "info",
    }
    .to_string()
}

/// `(protocol, method, path)` projected from the operation key. For non-HTTP
/// protocols `method` is `None` and `path` carries the field / event name.
fn project_key(key: &OperationKey) -> (String, Option<String>, Option<String>) {
    match key {
        OperationKey::Http { method, path } => {
            ("http".to_string(), Some(method.clone()), Some(path.clone()))
        }
        OperationKey::Graphql { field, .. } => ("graphql".to_string(), None, Some(field.clone())),
        OperationKey::Socket { event, .. } => ("socket".to_string(), None, Some(event.clone())),
    }
}

/// `file_path` is stored as `"<file>:<line>"` for deterministic output. Split it
/// back into a path and a best-effort line number (0 when absent/unparseable).
/// `file`/`line` are informational only — scoring keys off method + path.
fn split_location(p: &Path) -> (String, u32) {
    let s = p.to_string_lossy();
    match s.rsplit_once(':') {
        Some((file, line)) if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) => {
            (file.to_string(), line.parse().unwrap_or(0))
        }
        _ => (s.into_owned(), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{
        ApiIssues, ConflictSeverity, CrossRepoMatch as AnalyzerCrossRepoMatch, DependencyConflict,
        RepoPackageInfo,
    };
    use crate::cloud_storage::{
        ManifestRole, ManifestTypeKind, ManifestTypeState, TypeEvidence, TypeManifestEntry,
    };
    use crate::services::type_sidecar::InferKind;
    use serde_json::Value;
    use std::path::PathBuf;

    fn manifest_entry(
        key: OperationKey,
        type_state: ManifestTypeState,
        is_explicit: bool,
        resolved: Option<&str>,
        primary_type_symbol: Option<&str>,
    ) -> TypeManifestEntry {
        TypeManifestEntry {
            key,
            role: ManifestRole::Producer,
            type_kind: ManifestTypeKind::Response,
            type_alias: "Endpoint_abc_Response".to_string(),
            file_path: "src/orders.ts".to_string(),
            line_number: 12,
            is_explicit,
            type_state,
            evidence: TypeEvidence {
                file_path: "src/orders.ts".to_string(),
                span_start: None,
                span_end: None,
                line_number: 12,
                infer_kind: InferKind::ResponseBody,
                is_explicit,
                type_state,
            },
            resolved_definition: resolved.map(String::from),
            expanded_definition: None,
            primary_type_symbol: primary_type_symbol.map(String::from),
        }
    }

    fn empty_issues_with_deps(dependency_conflicts: Vec<DependencyConflict>) -> ApiIssues {
        ApiIssues {
            call_issues: vec![],
            endpoint_issues: vec![],
            env_var_calls: vec![],
            mismatches: vec![],
            type_mismatches: vec![],
            dependency_conflicts,
        }
    }

    fn endpoint(method: &str, path: &str, file_line: &str) -> ApiEndpointDetails {
        ApiEndpointDetails {
            owner: None,
            key: OperationKey::http(method, path.to_string()),
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: Some("getOrder".to_string()),
            request_type: None,
            response_type: None,
            file_path: PathBuf::from(file_line),
            repo_name: None,
            service_name: None,
        }
    }

    /// Build a projection with a cross-repo match, manifest-joined op fields,
    /// and a dependency conflict, then assert the serialized JSON shape.
    #[test]
    fn projection_serializes_full_cross_repo_shape() {
        let producer_key = OperationKey::http("GET", "/orders/:param");
        let endpoints = vec![endpoint("GET", "/orders/:param", "src/orders.ts:12")];

        let matches = vec![
            // Compat evaluated, compatible.
            AnalyzerCrossRepoMatch {
                producer_repo: "orders-monorepo".to_string(),
                producer_key: producer_key.canonical(),
                consumer_repo: "payments-svc".to_string(),
                consumer_key: "http|GET|/orders/:param".to_string(),
                match_score: 1.0,
                type_compatible: Some(true),
                mismatch_reason: None,
            },
            // Compat evaluated, incompatible.
            AnalyzerCrossRepoMatch {
                producer_repo: "orders-monorepo".to_string(),
                producer_key: producer_key.canonical(),
                consumer_repo: "web-frontend".to_string(),
                consumer_key: "http|GET|/orders/:param".to_string(),
                match_score: 1.0,
                type_compatible: Some(false),
                mismatch_reason: Some("id: number vs string".to_string()),
            },
            // Compat NOT evaluated (the load-bearing None).
            AnalyzerCrossRepoMatch {
                producer_repo: "orders-monorepo".to_string(),
                producer_key: "http|POST|/payments".to_string(),
                consumer_repo: "web-frontend".to_string(),
                consumer_key: "http|POST|/payments".to_string(),
                match_score: 1.0,
                type_compatible: None,
                mismatch_reason: None,
            },
        ];

        let deps = vec![DependencyConflict {
            package_name: "zod".to_string(),
            // Deliberately out of order to prove the projection sorts versions.
            repos: vec![
                RepoPackageInfo {
                    repo_name: "web-frontend".to_string(),
                    version: "3.23.0".to_string(),
                    source_path: PathBuf::from("web-frontend/package.json"),
                },
                RepoPackageInfo {
                    repo_name: "payments-svc".to_string(),
                    version: "3.22.0".to_string(),
                    source_path: PathBuf::from("payments-svc/package.json"),
                },
            ],
            severity: ConflictSeverity::Warning,
        }];

        let result = ApiAnalysisResult {
            endpoints,
            calls: vec![],
            issues: empty_issues_with_deps(deps),
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
            cross_repo_matches: matches,
        };

        let manifest = vec![manifest_entry(
            producer_key,
            ManifestTypeState::Explicit,
            true,
            Some("{ id: number; amountCents: number }"),
            Some("OrderResponse"),
        )];

        let projection = EvalProjection::from_results(&result, &manifest);
        let json: Value =
            serde_json::from_str(&serde_json::to_string(&projection).unwrap()).unwrap();

        // --- top-level keys present, snake_case ---
        assert!(json.get("cross_repo_matches").is_some());
        assert!(json.get("dependency_conflicts").is_some());

        // --- cross_repo_matches shape ---
        let cms = json["cross_repo_matches"].as_array().unwrap();
        assert_eq!(cms.len(), 3);

        let compatible = &cms[0];
        assert_eq!(compatible["producer_repo"], "orders-monorepo");
        assert_eq!(compatible["producer_key"], "http|GET|/orders/:param");
        assert_eq!(compatible["consumer_repo"], "payments-svc");
        assert_eq!(compatible["match_score"], 1.0);
        assert_eq!(compatible["type_compatible"], true);
        // mismatch_reason omitted when None.
        assert!(compatible.get("mismatch_reason").is_none());

        let incompatible = &cms[1];
        assert_eq!(incompatible["type_compatible"], false);
        assert_eq!(incompatible["mismatch_reason"], "id: number vs string");

        // type_compatible: None must be OMITTED, not serialized as null.
        let not_evaluated = &cms[2];
        assert!(
            not_evaluated.get("type_compatible").is_none(),
            "type_compatible: None must be omitted from JSON, got {:?}",
            not_evaluated
        );
        assert!(not_evaluated.get("mismatch_reason").is_none());

        // --- manifest-joined op fields ---
        let op = &json["endpoints"].as_array().unwrap()[0];
        assert_eq!(op["key"], "http|GET|/orders/:param");
        assert_eq!(op["type_state"], "Explicit");
        assert_eq!(op["is_explicit"], true);
        assert_eq!(
            op["resolved_definition"],
            "{ id: number; amountCents: number }"
        );
        assert_eq!(op["type_alias"], "Endpoint_abc_Response");
        // The real LLM anchor surfaces — NOT the hashed type_alias (#233).
        assert_eq!(op["primary_type_symbol"], "OrderResponse");
        // expanded_definition was None → omitted.
        assert!(op.get("expanded_definition").is_none());

        // --- dependency_conflicts shape: versions sorted ascending ---
        let dc = &json["dependency_conflicts"].as_array().unwrap()[0];
        assert_eq!(dc["package"], "zod");
        assert_eq!(
            dc["versions"].as_array().unwrap(),
            &vec![Value::from("3.22.0"), Value::from("3.23.0")]
        );
        assert_eq!(dc["severity"], "warning");
    }

    /// An op with no manifest entry must omit every manifest field (omit-if-none).
    #[test]
    fn op_without_manifest_omits_type_fields() {
        let result = ApiAnalysisResult {
            endpoints: vec![endpoint("GET", "/api/v1/status", "src/status.ts:3")],
            calls: vec![],
            issues: empty_issues_with_deps(vec![]),
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
            cross_repo_matches: vec![],
        };

        let projection = EvalProjection::from_results(&result, &[]);
        let json: Value =
            serde_json::from_str(&serde_json::to_string(&projection).unwrap()).unwrap();
        let op = &json["endpoints"].as_array().unwrap()[0];

        for field in [
            "type_alias",
            "type_state",
            "resolved_definition",
            "expanded_definition",
            "is_explicit",
            "primary_type_symbol",
        ] {
            assert!(
                op.get(field).is_none(),
                "{} must be omitted when there is no manifest entry, got {:?}",
                field,
                op
            );
        }
        // Empty match/dep arrays still serialize as present empty arrays.
        assert_eq!(json["cross_repo_matches"].as_array().unwrap().len(), 0);
        assert_eq!(json["dependency_conflicts"].as_array().unwrap().len(), 0);
    }

    /// #245 Phase 1: a manifest entry keyed by a *socket* OperationKey joins to
    /// its op in the projection and surfaces `primary_type_symbol`/`type_state`
    /// on the EvalOp — proving the projection join is protocol-agnostic, not
    /// HTTP-only. (Plan test #4.)
    #[test]
    fn socket_keyed_manifest_entry_surfaces_on_eval_op() {
        use crate::operation::SocketDirection;

        let socket_key = OperationKey::socket("payment:settled", SocketDirection::ServerToClient);
        // A socket emitter lands on `calls` (consumer side).
        let call = ApiEndpointDetails {
            owner: None,
            key: socket_key.clone(),
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: None,
            request_type: None,
            response_type: None,
            file_path: PathBuf::from("src/socket.ts:12"),
            repo_name: None,
            service_name: None,
        };

        let result = ApiAnalysisResult {
            endpoints: vec![],
            calls: vec![call],
            issues: empty_issues_with_deps(vec![]),
            verified_endpoints: vec![],
            detected_graphql_libraries: vec![],
            graphql_operations_indexed: false,
            cross_repo_matches: vec![],
        };

        let manifest = vec![manifest_entry(
            socket_key,
            ManifestTypeState::Explicit,
            true,
            Some("{ id: string; amountCents: number }"),
            Some("Payment"),
        )];

        let projection = EvalProjection::from_results(&result, &manifest);
        let json: Value =
            serde_json::from_str(&serde_json::to_string(&projection).unwrap()).unwrap();
        let op = &json["calls"].as_array().unwrap()[0];

        assert_eq!(op["key"], "socket|SERVER->CLIENT|payment:settled");
        assert_eq!(op["protocol"], "socket");
        // method is null for non-HTTP (the field is always present); path
        // carries the event name.
        assert_eq!(op["method"], Value::Null);
        assert_eq!(op["path"], "payment:settled");
        assert_eq!(op["type_state"], "Explicit");
        assert_eq!(op["primary_type_symbol"], "Payment");
        assert_eq!(
            op["resolved_definition"],
            "{ id: string; amountCents: number }"
        );
    }
}
