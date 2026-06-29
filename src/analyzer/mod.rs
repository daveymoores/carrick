pub mod builder;

use swc_common::{FileName, SourceMap, SourceMapper, Spanned, sync::Lrc};
use swc_ecma_ast::TsTypeAnn;

use crate::{
    app_context::AppContext,
    config::{Config, create_standard_tsconfig},
    extractor::CoreExtractor,
    mount_graph::MountGraph,
    operation::OperationKey,
    packages::Packages,
    type_manifest::parse_file_location,
    url_normalizer::UrlNormalizer,
    utils::join_prefix_and_path,
    visitor::{Call, FunctionDefinition, FunctionNodeType, Json, Mount, OwnerType, TypeReference},
};
use std::collections::HashSet;
use std::sync::LazyLock;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tracing::{debug, warn};

// Regexes are compiled once and reused across every endpoint/type-string pass.
static ROUTE_PARAM_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r":([\w]+)").unwrap());
static IMPORT_PATH_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"import\("([^"]+)"\)\.(\w+)"#).unwrap());
static ARRAY_GENERIC_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"Array<([^>]+)>").unwrap());

// Type aliases to reduce complexity
type RouteFieldMap = HashMap<OperationKey, Json>;
/// Result of `analyze_matches_with_mount_graph`:
///   `(call_issues, endpoint_issues, env_var_calls, verified_endpoints,
///     cross_repo_matches)`.
type MountGraphMatches = (
    Vec<String>,
    Vec<OrphanedEndpoint>,
    Vec<String>,
    Vec<(String, String)>,
    Vec<CrossRepoMatch>,
);
/// Result of `analyze_exact_key_matches` (the non-HTTP, exact-operation-key
/// matcher): `(call_issues, endpoint_issues, verified_endpoints,
/// cross_repo_matches)`. No `env_var_calls` slot — GraphQL/socket keys carry no
/// URL to classify.
type ExactKeyMatches = (
    Vec<String>,
    Vec<OrphanedEndpoint>,
    Vec<(String, String)>,
    Vec<CrossRepoMatch>,
);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum ConflictSeverity {
    Critical, // Major version differences (1.x vs 2.x)
    Warning,  // Minor version differences (1.1.x vs 1.2.x)
    Info,     // Patch version differences (1.1.1 vs 1.1.2)
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DependencyConflict {
    pub package_name: String,
    pub repos: Vec<RepoPackageInfo>,
    pub severity: ConflictSeverity,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RepoPackageInfo {
    pub repo_name: String,
    pub version: String,
    pub source_path: PathBuf,
}

/// A producer endpoint with no consumer in the indexed services. `service`
/// names the owning service (monorepo `serviceName`) or repo (poly-repo) when
/// known; it is `None` for protocols whose orphans are not repo-tagged
/// (GraphQL/socket).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct OrphanedEndpoint {
    pub method: String,
    pub path: String,
    pub service: Option<String>,
}

/// A structured producer→consumer edge captured at the matching site. This is
/// the load-bearing cross-repo signal the eval scorer reads (contract §2): an
/// endpoint in one repo matched by an outbound call in another (or the same)
/// repo, with the type-compatibility verdict for that producer endpoint.
///
/// `type_compatible == None` is deliberate and load-bearing: it means compat
/// was never evaluated for this edge (e.g. `ts_check_dir` was absent, so type
/// checking did not run), as distinct from `Some(true)` "evaluated, compatible".
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CrossRepoMatch {
    /// Repo id of the producer endpoint (service_name ?? repo_name).
    pub producer_repo: String,
    /// `OperationKey::canonical()` of the producer endpoint (mount-resolved path).
    pub producer_key: String,
    /// Repo id of the consumer call.
    pub consumer_repo: String,
    /// `OperationKey::canonical()` of the consumer call (URL-normalized path).
    pub consumer_key: String,
    /// Source location of the consumer call (`"<file>:<line>[:<col>]"`), the join
    /// key that attributes a per-pair compat verdict to THIS consumer rather than
    /// smearing one producer's first verdict across all its consumers (#260). It
    /// shares the consumer manifest entry's source — both derive from the same
    /// call `file_location` — so after `parse_file_location` normalization the
    /// edge and the ts_check `consumerLocation` agree on `(path, line)`. Set for
    /// every edge a consumer call backs, HTTP and exact-key protocol edges
    /// alike: both constructors fill it from the call's `file_path`, and the
    /// overlay iterates all of them. A non-HTTP producer key simply leaves
    /// `type_compatible` `None` (ts_check is HTTP-only); the location is still
    /// recorded. `Option` only to leave room for an edge source with no call.
    pub consumer_location: Option<String>,
    /// Matcher confidence in `[0, 1]`. `1.0` for an exact normalized-key match
    /// (the only kind captured today; there is no finer score yet).
    pub match_score: f64,
    /// `None` = compat NOT evaluated for this edge; `Some(b)` = evaluated.
    pub type_compatible: Option<bool>,
    /// `Some(..)` iff `type_compatible == Some(false)`; human-readable reason.
    pub mismatch_reason: Option<String>,
}

pub struct ApiIssues {
    pub call_issues: Vec<String>,
    pub endpoint_issues: Vec<OrphanedEndpoint>,
    pub env_var_calls: Vec<String>,
    pub mismatches: Vec<String>,
    pub type_mismatches: Vec<String>,
    pub dependency_conflicts: Vec<DependencyConflict>,
}

impl ApiIssues {
    pub fn is_empty(&self) -> bool {
        self.call_issues.is_empty()
            && self.endpoint_issues.is_empty()
            && self.env_var_calls.is_empty()
            && self.mismatches.is_empty()
            && self.type_mismatches.is_empty()
            && self.dependency_conflicts.is_empty()
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ApiEndpointDetails {
    // owner is Option as we store both call ands endpoints in this data structure.
    // It might make sense to split this out into its own type
    pub owner: Option<OwnerType>,
    pub key: OperationKey,
    #[allow(dead_code)]
    pub params: Vec<String>,
    // - For endpoints, `request_body` is what the server expects to receive
    // - For calls, `request_body` is what the client is sending
    // - For endpoints, `response_body` is what the server sends back
    // - For calls, `response_body` is what the client expects to receive
    pub request_body: Option<Json>,
    pub response_body: Option<Json>,
    pub handler_name: Option<String>,
    pub request_type: Option<TypeReference>,
    pub response_type: Option<TypeReference>,
    pub file_path: PathBuf,
    /// Owning repo, stamped during the cross-repo merge from
    /// `CloudRepoData::repo_name`. `None` outside cross-repo mode (single-repo
    /// data is not repo-tagged). Non-HTTP (GraphQL/socket) matching reads this to
    /// attribute a matched producer/consumer pair to its repos for a
    /// `CrossRepoMatch` edge — HTTP ops get repo identity from the repo-tagged
    /// mount graph instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,
    /// Owning service (monorepo `serviceName`), stamped during the cross-repo
    /// merge. Preferred over `repo_name` for the edge repo id (matches the
    /// cloud's `service_name ?? repo_name` convention). `None` when no
    /// `serviceName`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

pub struct ApiAnalysisResult {
    pub endpoints: Vec<ApiEndpointDetails>,
    pub calls: Vec<ApiEndpointDetails>,
    pub issues: ApiIssues,
    /// Endpoints that were successfully matched by at least one consumer
    /// call, with their method + canonical path. Surfaced so users see
    /// what *worked* in the PR comment, not just what's broken — clean
    /// runs otherwise produce no positive signal.
    pub verified_endpoints: Vec<(String, String)>,
    /// GraphQL libraries detected across all scanned repos (subset of
    /// `detected_data_fetchers`). When libraries are present but no
    /// operations were extracted, the formatter suggests committing an
    /// emitted schema (code-first schemas and Relay artifacts are not
    /// statically extractable).
    pub detected_graphql_libraries: Vec<String>,
    /// Whether any GraphQL operations (schema fields or documents) made it
    /// into the index. Gates the "no GraphQL extracted" banner.
    pub graphql_operations_indexed: bool,
    /// Structured producer→consumer edges captured at the matching site, with
    /// per-edge type-compat verdicts. Populated by `get_results`; consumed by
    /// the eval projection (it has no effect on the human Markdown report).
    pub cross_repo_matches: Vec<CrossRepoMatch>,
}

/// Return the subset of `data_fetchers` that are GraphQL libraries.
/// Comparison is case-insensitive to handle package-name casing variations.
pub fn filter_graphql_libraries(data_fetchers: &[String]) -> Vec<String> {
    // Known GraphQL client/server libraries per framework-coverage.md §4.3.
    // Match against the lowercased package name — substring or equality.
    data_fetchers
        .iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            lower == "graphql"
                || lower == "graphql-request"
                || lower == "graphql-tag"
                || lower == "relay-runtime"
                || lower.starts_with("@apollo/")
                || lower.starts_with("@urql/")
                || lower == "urql"
                || lower == "apollo-client"
                || lower == "apollo-server"
        })
        .cloned()
        .collect()
}

/// Parse a ts_check compat `endpoint` string back into the verdict-join
/// `(pseudo-method, identity)` pair. ts_check builds the string as
/// `"<METHOD> <path> (<request|response>)"` for HTTP and
/// `"SOCKET <DIRECTION>|<event> (response)"` for socket, so the same split —
/// leading token off the front, trailing `" (type_kind)"` off the back — yields
/// `("METHOD", "path")` or `("SOCKET", "DIRECTION|event")`, matching what
/// `parse_producer_key` recovers from the edge's `producer_key`. Returns `None`
/// for an unrecognized shape.
fn parse_compat_endpoint(endpoint: &str) -> Option<(String, String)> {
    let (method, rest) = endpoint.split_once(' ')?;
    // Drop the trailing " (request)" / " (response)" annotation if present.
    let path = match rest.rfind(" (") {
        Some(idx) if rest.ends_with(')') => &rest[..idx],
        _ => rest,
    };
    if method.is_empty() || path.is_empty() {
        return None;
    }
    Some((method.to_uppercase(), path.to_string()))
}

/// Recover the verdict-join `(pseudo-method, identity)` from a canonical
/// producer key, for the protocols ts_check type-checks (HTTP + socket + graphql):
///
/// - HTTP (`"http|METHOD|path"`) → `("METHOD", "path")`, the HTTP join key.
/// - Socket (`"socket|DIRECTION|event"`) → `("SOCKET", "DIRECTION|event")`. The
///   matching ts_check endpoint label is `"SOCKET DIRECTION|event (response)"`,
///   which `parse_compat_endpoint` reduces to the SAME pair, so a socket edge
///   joins its ts_check verdict exactly like an HTTP one.
/// - GraphQL (`"graphql|KIND|field"`) → `("GRAPHQL", "KIND|field")`. The
///   matching ts_check endpoint label is `"GRAPHQL KIND|field (response)"`,
///   which `parse_compat_endpoint` reduces to the SAME pair, so a graphql edge
///   joins its verdict exactly like socket. The `KIND` (`query`/`mutation`/
///   `subscription`) stays lowercase here AND in the ts_check label, so the two
///   sides agree without any case folding.
///
/// Returns `None` for any other protocol: ts_check produced no verdict for it,
/// so its edge stays `None` rather than fabricating one.
fn parse_producer_key(key: &str) -> Option<(String, String)> {
    let mut parts = key.splitn(3, '|');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("http"), Some(method), Some(path)) if !method.is_empty() && !path.is_empty() => {
            Some((method.to_uppercase(), path.to_string()))
        }
        (Some("socket"), Some(direction), Some(event))
            if !direction.is_empty() && !event.is_empty() =>
        {
            Some(("SOCKET".to_string(), format!("{}|{}", direction, event)))
        }
        (Some("graphql"), Some(kind), Some(field)) if !kind.is_empty() && !field.is_empty() => {
            Some(("GRAPHQL".to_string(), format!("{}|{}", kind, field)))
        }
        _ => None,
    }
}

/// Canonicalize a consumer source location (`"<file>:<line>[:<col>]"`) to the
/// `(path, line)` pair that joins a `CrossRepoMatch` edge to its ts_check
/// per-pair verdict (#260). Both sides feed the same `call.file_location` here:
/// the edge stores it verbatim (`path:line:col`), while ts_check reassembles
/// `consumerLocation` as `parse_file_location(...).path : line`. Reducing both
/// through `parse_file_location` strips the divergent column/format suffix so
/// the verdict for one consumer can no longer smear onto another consumer of the
/// same producer endpoint.
fn consumer_identity(location: &str) -> (String, u32) {
    parse_file_location(location)
}

/// Collapse any dynamic path segment (`:id`, `{id}`, `[id]`) to `:param` so the
/// compat verdict join is param-NAME-agnostic. The cross-repo edge's
/// `producer_key` keeps the source param name (`/orders/:id`), while ts_check's
/// `endpoint` is built from the normalized manifest (`/orders/:param`). Without
/// collapsing BOTH sides, the join misses on every parameterized route and the
/// edge falls back to the optimistic `Some(true)` default — the live cause of
/// compat being pinned regardless of the actual ts_check verdicts.
fn normalize_compat_path(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            let is_param = seg.starts_with(':')
                || (seg.starts_with('{') && seg.ends_with('}'))
                || (seg.starts_with('[') && seg.ends_with(']'));
            if is_param { ":param" } else { seg }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Map ts_check's compatibility `result` onto each cross-repo edge's
/// `type_compatible`, keyed per consumer (#260) and param-name-agnostic on the
/// path ([`normalize_compat_path`]). Pure over `(result, matches)` so the
/// verdict join is unit-testable without spawning ts_check.
fn apply_compat_verdicts(result: &serde_json::Value, matches: &mut [CrossRepoMatch]) {
    // The per-pair verdict key: producer `(METHOD, normalized path)` plus the
    // consumer identity `(path, line)` recovered from `consumerLocation`.
    type VerdictKey = (String, String, (String, u32));

    // Map verdict key → mismatch reason. Multiple type_kinds for one pair
    // collapse to the first reason seen — the edge only records incompatibility.
    let mut incompatible: HashMap<VerdictKey, String> = HashMap::new();
    if let Some(mismatch_list) = result.get("mismatches").and_then(|m| m.as_array()) {
        for mismatch in mismatch_list {
            let Some(endpoint) = mismatch.get("endpoint").and_then(|e| e.as_str()) else {
                continue;
            };
            let Some((method, path)) = parse_compat_endpoint(endpoint) else {
                continue;
            };
            let Some(consumer) = mismatch
                .get("consumerLocation")
                .and_then(|c| c.as_str())
                .map(consumer_identity)
            else {
                continue;
            };
            let reason = mismatch
                .get("error")
                .and_then(|e| e.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("producer and consumer types are incompatible")
                .to_string();
            incompatible
                .entry((method, normalize_compat_path(&path), consumer))
                .or_insert(reason);
        }
    }

    // Pairs ts_check matched but could not verify (a side resolved to
    // `any`/`unknown`). The compat verdict is genuinely unknown for these edges —
    // leaving the optimistic `Some(true)` default would assert a compatibility
    // ts_check never established.
    let mut unverifiable: HashSet<VerdictKey> = HashSet::new();
    if let Some(unknown_list) = result.get("unknownPairs").and_then(|u| u.as_array()) {
        for pair in unknown_list {
            let Some(endpoint) = pair.get("endpoint").and_then(|e| e.as_str()) else {
                continue;
            };
            let Some((method, path)) = parse_compat_endpoint(endpoint) else {
                continue;
            };
            let Some(consumer) = pair
                .get("consumerLocation")
                .and_then(|c| c.as_str())
                .map(consumer_identity)
            else {
                continue;
            };
            unverifiable.insert((method, normalize_compat_path(&path), consumer));
        }
    }

    for edge in matches.iter_mut() {
        // Recover the join key from the producer_key. ts_check type-checks HTTP
        // (`http|METHOD|path`), socket (`socket|DIRECTION|event`), and graphql
        // (`graphql|KIND|field`), so all three join here. A key for any other
        // protocol is not checked by ts_check, so its verdict is genuinely
        // unknown — leave it `None` rather than fabricate `Some(true)`
        // (#260, part 2).
        let Some((method, path)) = parse_producer_key(&edge.producer_key) else {
            edge.type_compatible = None;
            continue;
        };
        // Without a consumer identity the pair can't be matched to its own
        // verdict, and asserting `Some(true)` would risk re-smearing — leave it
        // `None` (compat undetermined for this edge).
        let Some(consumer) = edge.consumer_location.as_deref().map(consumer_identity) else {
            edge.type_compatible = None;
            continue;
        };
        let key = (method, normalize_compat_path(&path), consumer);
        if let Some(reason) = incompatible.get(&key) {
            edge.type_compatible = Some(false);
            edge.mismatch_reason = Some(reason.clone());
        } else if unverifiable.contains(&key) {
            // Matched but unverifiable — compat undetermined, NOT compatible.
            edge.type_compatible = None;
        } else {
            edge.type_compatible = Some(true);
        }
    }
}

/// Sort cross-repo edges into a deterministic order and drop exact duplicates,
/// keyed on the
/// `(producer_repo, producer_key, consumer_repo, consumer_key, consumer_location)`
/// identity tuple. The HTTP matcher and the non-HTTP matcher capture edges in
/// non-deterministic iteration order, and `get_results` re-runs this over the
/// combined set so every consumer (PR comment, dashboard, eval projection,
/// cassette gate) sees a stable order. `consumer_location` is part of the
/// identity so two distinct call sites in one consumer repo against the same
/// producer endpoint stay separate edges (each carries its own verdict).
fn sort_dedup_cross_repo_matches(matches: &mut Vec<CrossRepoMatch>) {
    matches.sort_by(|a, b| {
        (
            &a.producer_repo,
            &a.producer_key,
            &a.consumer_repo,
            &a.consumer_key,
            &a.consumer_location,
        )
            .cmp(&(
                &b.producer_repo,
                &b.producer_key,
                &b.consumer_repo,
                &b.consumer_key,
                &b.consumer_location,
            ))
    });
    matches.dedup_by(|a, b| {
        a.producer_repo == b.producer_repo
            && a.producer_key == b.producer_key
            && a.consumer_repo == b.consumer_repo
            && a.consumer_key == b.consumer_key
            && a.consumer_location == b.consumer_location
    });
}

/// Accept only values whose *shape* is an extractable outgoing-call route, as
/// produced by the file-analyzer LLM's `target` field (see that lambda's
/// system prompt): an absolute path (`/users`), a full URL (`http(s)://…`), or
/// an env-var base form (`${VAR}/path`, `${process.env.VAR}/…`). Template
/// params like `${id}` are legal *inside* a path and are preserved.
///
/// Everything else — bare identifiers (`query`, `DynamoDB`, `CarrickApiKeys`),
/// member/call expressions (`res.json()`, `params.service`), SDK operation
/// tokens (`Service:Op`, `Service.Op`), and literals (`null`, `new`, `.`,
/// `unknown`) — is rejected. Pure string-shape logic: it names no framework,
/// client, or SDK. The shape-blind residue (a bare `${TABLE_NAME}` that is
/// really a datastore resource, not a base URL) is handled on the prompt side.
pub fn is_valid_route_shape(route: &str) -> bool {
    let route = route.trim();
    if route.is_empty() {
        return false;
    }
    // No leftover JavaScript-source markers that prove the value is an
    // unresolved expression (`a || b`, a call/group) rather than a route.
    let is_clean = |s: &str| {
        !s.contains("||")
            && !s.contains('(')
            && !s.contains(')')
            && !s.chars().any(|c| c.is_whitespace())
    };

    // Explicit `ENV_VAR:NAME:/path` form (the analyzer's canonical env-var
    // route; see `is_env_var_base_url` / `extract_env_var_name`).
    if let Some(rest) = route.strip_prefix("ENV_VAR:") {
        let mut parts = rest.splitn(2, ':');
        let name = parts.next().unwrap_or("");
        return match parts.next() {
            Some(path) => {
                !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && path.starts_with('/')
                    && is_clean(path)
                    && path.trim_start_matches('/') != name
            }
            None => false,
        };
    }
    // Env-var base form: `${VAR}/path`, `${process.env.VAR}/…`, bare `${VAR}`.
    if let Some(rest) = route.strip_prefix("${") {
        return rest.contains('}') && is_clean(route);
    }
    // Full URL.
    if route.starts_with("http://") || route.starts_with("https://") {
        return is_clean(route);
    }
    // Absolute path (may carry `${id}` / `:id` template params).
    if route.starts_with('/') {
        return is_clean(route);
    }
    // Bare identifier, member/call expression, `Service:Op`, `#…`, literal.
    false
}

pub struct Analyzer {
    // <Route, http_method, handler_name, source>
    pub imported_handlers: Vec<(String, String, String, String)>,
    pub function_definitions: HashMap<String, FunctionDefinition>,
    pub endpoints: Vec<ApiEndpointDetails>,
    pub calls: Vec<ApiEndpointDetails>,
    fetch_calls: Vec<Call>, // Store processed fetch calls with unique IDs
    pub mounts: Vec<Mount>,
    pub apps: HashMap<String, AppContext>,
    config: Config,
    endpoint_router: Option<matchit::Router<Vec<(String, String)>>>,
    source_map: Lrc<SourceMap>,
    all_repo_packages: HashMap<String, Packages>, // repo_name -> packages
    detected_frameworks: Vec<String>,
    detected_data_fetchers: Vec<String>,
    mount_graph: Option<MountGraph>, // Mount graph for framework-agnostic analysis
    ts_check_dir: Option<PathBuf>,   // Resolved ts_check/ directory; set by the CLI entry point
}

impl CoreExtractor for Analyzer {
    fn get_source_map(&self) -> &Lrc<SourceMap> {
        &self.source_map
    }
}

impl Analyzer {
    pub fn new(config: Config, source_map: Lrc<SourceMap>) -> Self {
        Analyzer {
            imported_handlers: Vec::new(),
            function_definitions: HashMap::new(),
            endpoints: Vec::new(),
            calls: Vec::new(),
            fetch_calls: Vec::new(),
            mounts: Vec::new(),
            apps: HashMap::new(),
            config,
            endpoint_router: None,
            source_map,
            all_repo_packages: HashMap::new(),
            detected_frameworks: Vec::new(),
            detected_data_fetchers: Vec::new(),
            mount_graph: None,
            ts_check_dir: None,
        }
    }

    /// Set the mount graph for framework-agnostic analysis
    pub fn set_mount_graph(&mut self, mount_graph: MountGraph) {
        self.mount_graph = Some(mount_graph);
    }

    /// Set the resolved ts_check/ directory. The CLI entry point discovers this
    /// via `discover_ts_check_path`; tests and callers that don't need type
    /// checking can leave it unset.
    pub fn set_ts_check_dir(&mut self, ts_check_dir: PathBuf) {
        self.ts_check_dir = Some(ts_check_dir);
    }

    fn ts_check_output_dir(&self) -> Option<PathBuf> {
        self.ts_check_dir.as_ref().map(|d| d.join("output"))
    }

    pub fn add_repo_packages(&mut self, repo_name: String, packages: Packages) {
        self.all_repo_packages.insert(repo_name, packages);
    }

    #[allow(dead_code)]
    pub fn set_framework_detection(&mut self, frameworks: Vec<String>, data_fetchers: Vec<String>) {
        self.detected_frameworks = frameworks;
        self.detected_data_fetchers = data_fetchers;
    }

    pub fn analyze_dependencies(&self) -> Vec<DependencyConflict> {
        self.find_dependency_conflicts()
    }

    fn find_dependency_conflicts(&self) -> Vec<DependencyConflict> {
        let mut package_versions: HashMap<String, Vec<RepoPackageInfo>> = HashMap::new();

        // Collect all packages from all repositories
        for (repo_name, packages) in &self.all_repo_packages {
            for (package_name, package_info) in packages.get_dependencies() {
                let repo_package_info = RepoPackageInfo {
                    repo_name: repo_name.clone(),
                    version: package_info.version.clone(),
                    source_path: package_info.source_path.clone(),
                };

                package_versions
                    .entry(package_name.clone())
                    .or_default()
                    .push(repo_package_info);
            }
        }

        // Find packages with conflicting versions
        let mut conflicts = Vec::new();
        for (package_name, repo_infos) in package_versions {
            if repo_infos.len() > 1 {
                // Check if all versions are the same
                let first_version = &repo_infos[0].version;
                let has_conflicts = repo_infos.iter().any(|info| info.version != *first_version);

                if has_conflicts && Self::is_reportable_conflict(&repo_infos) {
                    let severity = Self::determine_conflict_severity(&repo_infos);
                    conflicts.push(DependencyConflict {
                        package_name,
                        repos: repo_infos,
                        severity,
                    });
                }
            }
        }

        conflicts
    }

    /// A cross-service version difference is only worth reporting when the
    /// versions are semver-INCOMPATIBLE — i.e. they span more than one MAJOR
    /// version (`zod` 3.x vs 4.x). Differences confined to minor/patch within a
    /// single major (`typescript` 5.3 vs 5.4) are semver-compatible by
    /// construction and would be false positives: they don't cause the
    /// cross-service type/runtime breakage this report exists to surface, and on
    /// real org-wide installs they are pervasive noise that buries the genuine
    /// major-version conflicts. Versions that don't parse as semver fall back to
    /// "report it" — `has_conflicts` already established the raw strings differ,
    /// and a genuinely divergent non-semver pin (`workspace:*` vs a tag, a git
    /// URL) must never be silently dropped.
    fn is_reportable_conflict(repo_infos: &[RepoPackageInfo]) -> bool {
        use semver::Version;

        let parsed: Vec<Version> = repo_infos
            .iter()
            .filter_map(|info| Version::parse(&info.version).ok())
            .collect();

        // At least one version is not valid semver: report conservatively.
        if parsed.len() != repo_infos.len() {
            return true;
        }

        let first_major = parsed[0].major;
        parsed.iter().any(|v| v.major != first_major)
    }

    fn determine_conflict_severity(repo_infos: &[RepoPackageInfo]) -> ConflictSeverity {
        use semver::Version;

        let mut versions = Vec::new();
        for info in repo_infos {
            if let Ok(version) = Version::parse(&info.version) {
                versions.push(version);
            }
        }

        if versions.len() < 2 {
            return ConflictSeverity::Info;
        }

        // Check for major version differences
        let first_major = versions[0].major;
        if versions.iter().any(|v| v.major != first_major) {
            return ConflictSeverity::Critical;
        }

        // Check for minor version differences
        let first_minor = versions[0].minor;
        if versions.iter().any(|v| v.minor != first_minor) {
            return ConflictSeverity::Warning;
        }

        // Only patch differences remain
        ConflictSeverity::Info
    }

    pub async fn analyze_functions_for_fetch_calls(&mut self) {
        use crate::agent_service::extract_calls_from_async_expressions;

        let mut all_async_contexts = Vec::new();

        // Extract async calls from each function definition using extractor methods
        for def in self.function_definitions.values() {
            let async_contexts = self.extract_async_calls_from_function(def);
            all_async_contexts.extend(async_contexts);
        }

        // Skip Gemini call if no async expressions found (safety check)
        if all_async_contexts.is_empty() {
            debug!("No async expressions found, skipping Gemini analysis");
            return;
        }

        // Send to Gemini Flash 2.5 for analysis with framework context
        let gemini_calls = match extract_calls_from_async_expressions(
            all_async_contexts,
            &self.detected_frameworks,
            &self.detected_data_fetchers,
        )
        .await
        {
            Ok(calls) => calls,
            Err(e) => {
                warn!("Failed to extract calls from async expressions: {}", e);
                vec![]
            }
        };

        debug!("Gemini extracted {} HTTP calls", gemini_calls.len());

        // Process calls as before
        let processed_calls = self.process_fetch_calls(gemini_calls);
        self.fetch_calls.extend(processed_calls.clone());

        // Create ApiEndpointDetails from processed calls
        for call in processed_calls {
            let params = self.extract_params_from_route(&call.route);
            self.calls.push(ApiEndpointDetails {
                owner: None,
                key: OperationKey::http(&call.method, call.route.clone()),
                params,
                request_body: call.request.clone(),
                response_body: Some(Json::Null),
                handler_name: None,
                request_type: call.request_type.clone(),
                response_type: call.response_type.clone(),
                file_path: call.call_file.clone(),
                repo_name: None,
                service_name: None,
            });
        }
    }

    fn byte_offset_to_utf16_offset(source: &str, byte_offset: usize) -> usize {
        source[..byte_offset].encode_utf16().count()
    }

    /// Normalize route by removing ENV_VAR prefixes and extracting the actual path
    fn normalize_route_for_type_name(route: &str) -> String {
        if route.contains("ENV_VAR:") {
            // Extract the actual path from ENV_VAR constructs
            // "ENV_VAR:COMMENT_SERVICE_URL:/api/comments" -> "/api/comments"
            let segments: Vec<&str> = route.split("ENV_VAR:").collect();
            let mut clean_path = String::new();

            // Add the part before any ENV_VAR marker
            clean_path.push_str(segments[0]);

            // Process each segment with an ENV_VAR marker
            for segment in segments.iter().skip(1) {
                let subparts: Vec<&str> = segment.splitn(2, ':').collect();
                if subparts.len() == 2 {
                    clean_path.push_str(subparts[1]);
                }
            }

            clean_path
        } else {
            route.to_string()
        }
    }

    /// Generate common type alias name for producer/consumer comparison
    /// This creates matching names that can be compared via ts-morph
    pub fn generate_common_type_alias_name(
        route: &str,
        method: &str,
        is_request_type: bool,
        is_consumer: bool,
    ) -> String {
        let suffix = if is_request_type {
            "Request"
        } else {
            "Response"
        };
        let role = if is_consumer { "Consumer" } else { "Producer" };
        let method_pascal = Self::method_to_pascal_case(method);

        // Normalize the route to handle env vars consistently
        let normalized_route = Self::normalize_route_for_type_name(route);
        let sanitized_route = Self::sanitize_route_for_dynamic_paths(&normalized_route);

        format!("{}{}{}{}", method_pascal, sanitized_route, suffix, role)
    }

    /// Generate unique type alias name for tracking individual calls
    /// This is used internally for analysis but not for type comparison
    pub fn generate_unique_call_alias_name(
        route: &str,
        method: &str,
        is_request_type: bool,
        call_number: u32,
        is_consumer: bool,
    ) -> String {
        let suffix = if is_request_type {
            "Request"
        } else {
            "Response"
        };
        let role = if is_consumer { "Consumer" } else { "Producer" };
        let method_pascal = Self::method_to_pascal_case(method);
        let sanitized_route = Self::sanitize_route_for_dynamic_paths(route);
        format!(
            "{}{}{}{}Call{}",
            method_pascal, sanitized_route, suffix, role, call_number
        )
    }

    /// Helper method to convert HTTP method to PascalCase
    fn method_to_pascal_case(method: &str) -> String {
        if method.is_empty() {
            "UnknownMethod".to_string()
        } else {
            let lowercase_method = method.to_lowercase();
            let mut m = lowercase_method.chars();
            match m.next() {
                None => "UnknownMethod".to_string(),
                Some(f) => f.to_uppercase().collect::<String>() + m.as_str(),
            }
        }
    }

    /// Process fetch calls and assign unique identifiers and common type names
    pub fn process_fetch_calls(&mut self, mut calls: Vec<Call>) -> Vec<Call> {
        // Group calls by route+method to ensure consecutive numbering
        let mut grouped_calls: std::collections::HashMap<(String, String), Vec<usize>> =
            std::collections::HashMap::new();

        // Group call indices by route+method, but only for calls that have response_type
        for (index, call) in calls.iter().enumerate() {
            if call.response_type.is_some() {
                let key = (call.route.clone(), call.method.clone());
                grouped_calls.entry(key).or_default().push(index);
            }
        }

        // Process each group and assign consecutive numbers
        for ((route, method), indices) in grouped_calls {
            for (position, &call_index) in indices.iter().enumerate() {
                let call_number = (position + 1) as u32; // Start from 1
                let call = &mut calls[call_index];

                // Set unique call ID for tracking
                call.call_id = Some(Self::generate_unique_call_alias_name(
                    &route,
                    &method,
                    false, // is_request_type = false (for response)
                    call_number,
                    true, // is_consumer = true (fetch calls are consumers)
                ));

                // Set call number
                call.call_number = Some(call_number);

                // Set common type name for comparison with producer
                call.common_type_name = Some(Self::generate_common_type_alias_name(
                    &route, &method, false, // is_request_type = false (for response)
                    true,  // is_consumer = true (fetch calls are consumers)
                ));

                // Update TypeReference objects with unique aliases
                if let Some(ref mut response_type) = call.response_type {
                    response_type.alias = Self::generate_unique_call_alias_name(
                        &route,
                        &method,
                        false, // is_request_type = false (for response)
                        call_number,
                        true, // is_consumer = true (fetch calls are consumers)
                    );
                }

                if let Some(ref mut request_type) = call.request_type {
                    request_type.alias = Self::generate_unique_call_alias_name(
                        &route,
                        &method,
                        true, // is_request_type = true (for request)
                        call_number,
                        true, // is_consumer = true (fetch calls are consumers)
                    );
                }
            }
        }
        calls
    }

    fn sanitize_route_for_dynamic_paths(route: &str) -> String {
        // Strip query parameters first
        let route_without_query = if let Some(query_idx) = route.find('?') {
            &route[..query_idx]
        } else {
            route
        };

        route_without_query
            .split('/')
            .filter(|segment| !segment.is_empty()) // Remove empty segments
            .map(|segment| {
                if let Some(param_name) = segment.strip_prefix(':') {
                    // Convert :id -> ById, :userId -> ByUserId, :eventId -> ByEventId
                    format!("By{}", Self::to_pascal_case(param_name))
                } else if segment.starts_with("${") && segment.ends_with('}') {
                    // Handle template literal syntax: ${userId} -> ByUserid
                    // Extract the variable name from ${varName} or ${process.env.VAR}
                    let inner = &segment[2..segment.len() - 1]; // Remove ${ and }
                    // If it contains a dot (like process.env.VAR), take the last part
                    let param_name = inner.rsplit('.').next().unwrap_or(inner);
                    format!("By{}", Self::to_pascal_case(param_name))
                } else {
                    // Convert regular segments to PascalCase
                    Self::to_pascal_case(segment)
                }
            })
            .collect::<Vec<String>>()
            .join("")
    }

    fn to_pascal_case(input: &str) -> String {
        if input.is_empty() {
            return String::new();
        }

        let mut result = String::new();
        let mut capitalize_next = true;

        for ch in input.chars() {
            if ch.is_alphanumeric() {
                if capitalize_next {
                    result.push(ch.to_uppercase().next().unwrap_or(ch));
                    capitalize_next = false;
                } else {
                    result.push(ch.to_lowercase().next().unwrap_or(ch));
                }
            } else {
                // Non-alphanumeric characters trigger capitalization of next char
                capitalize_next = true;
            }
        }

        result
    }

    /// Extract environment variable name from a route
    /// Examples:
    /// - "ENV_VAR:API_URL:/users" -> "API_URL"
    /// - "${process.env.SERVICE_URL}/orders" -> "SERVICE_URL"
    /// - "${API_BASE}/users" -> "API_BASE"
    /// - "unknown" -> "UNKNOWN_API"
    fn extract_env_var_name(route: &str) -> String {
        // Handle ENV_VAR:NAME:/path format
        if route.starts_with("ENV_VAR:") {
            let parts: Vec<&str> = route.splitn(3, ':').collect();
            if parts.len() >= 2 {
                return parts[1].to_string();
            }
        }

        // Handle ${process.env.VAR} or ${VAR} patterns
        if let Some(start) = route.find("${")
            && let Some(end) = route[start..].find('}')
        {
            let inner = &route[start + 2..start + end];
            // Handle process.env.VAR -> VAR
            if let Some(last_dot) = inner.rfind('.') {
                return inner[last_dot + 1..].to_string();
            }
            return inner.to_string();
        }

        // Handle process.env.VAR patterns (without ${})
        if let Some(idx) = route.find("process.env.") {
            let after = &route[idx + 12..];
            let end = after
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after.len());
            if end > 0 {
                return after[..end].to_string();
            }
        }

        // Handle start-of-string variable (e.g. API_URL + "/path")
        if let Some(first_char) = route.chars().next()
            && first_char.is_uppercase()
        {
            let end = route
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(route.len());
            if end > 0 {
                return route[..end].to_string();
            }
        }

        "UNKNOWN_API".to_string()
    }

    /// Check if a route represents an environment variable base URL.
    ///
    /// Returns true for:
    /// - "ENV_VAR:API_URL:/users" (explicit ENV_VAR format)
    /// - "${process.env.API_URL}/users" (process.env pattern at start)
    /// - "${API_BASE_URL}/users" (UPPER_CASE var at start)
    ///
    /// Returns false for:
    /// - "/users/${userId}" (path parameter, not base URL)
    /// - "/api/${version}/data" (path parameter in middle)
    fn is_env_var_base_url(route: &str) -> bool {
        // Check for explicit ENV_VAR: prefix format
        if route.starts_with("ENV_VAR:") {
            return true;
        }

        // Check for process.env pattern
        if route.contains("process.env.") {
            return true;
        }

        // Check for ${...} at the START of the route (not in the middle)
        if route.starts_with("${")
            && let Some(end) = route.find('}')
        {
            let var_name = &route[2..end];
            // If it contains a dot (like process.env.X) or is UPPER_CASE, it's an env var
            if var_name.contains('.')
                || var_name
                    .chars()
                    .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
            {
                return true;
            }
        }

        // Check for start-of-string variables (e.g. API_URL + "/path")
        // If it starts with an uppercase letter and is not a path (doesn't start with /),
        // we treat it as a potential environment variable or constant base URL.
        if let Some(first_char) = route.chars().next()
            && first_char.is_uppercase()
        {
            // Extract the first identifier
            let end = route
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(route.len());

            // If the identifier is non-empty and looks like a constant (mostly uppercase/digits/underscore)
            // we treat it as an env var.
            // We verify it's at least 2 chars to avoid single letters being treated as vars excessively
            if end >= 2 {
                let ident = &route[..end];
                if ident
                    .chars()
                    .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
                {
                    return true;
                }
            }
        }

        false
    }

    /// Helper to process a TsTypeAnn and produce a TypeReference.
    /// This function encapsulates the logic to find the correct span,
    /// calculate the UTF-16 offset, and build the TypeReference struct.
    pub fn create_type_reference_from_swc(
        type_ann_swc: &TsTypeAnn,
        cm: &Lrc<SourceMap>,
        func_def_file_path: &Path,
        alias: String,
    ) -> Option<TypeReference> {
        let type_ref_span = match &*type_ann_swc.type_ann {
            swc_ecma_ast::TsType::TsTypeRef(type_ref) => type_ref.span,
            _ => type_ann_swc.span, // fallback
        };

        let loc = cm.lookup_char_pos(type_ref_span.lo);
        let file_start_bytepos = loc.file.start_pos;
        if type_ref_span.lo < file_start_bytepos {
            warn!(
                "Span `lo` ({:?}) is before its supposed file's start_pos ({:?}) for file {:?}. This indicates a SourceMap or span issue.",
                type_ref_span.lo, file_start_bytepos, loc.file.name
            );
            return None; // Or handle as an error appropriately
        }
        let file_relative_byte_offset_u32 = (type_ref_span.lo - file_start_bytepos).0;

        let actual_span_file_path = match &*loc.file.name {
            FileName::Real(pathbuf) => pathbuf.clone(), // Clone to own PathBuf
            other => {
                warn!(
                    "Span found in a non-real file: {:?}. Cannot process.",
                    other
                );
                return None;
            }
        };

        let file_content = match std::fs::read_to_string(&actual_span_file_path) {
            Ok(content) => content,
            Err(e) => {
                warn!(
                    "Failed to read file {:?} for offset calculation: {}. Skipping.",
                    actual_span_file_path, e
                );
                return None;
            }
        };

        let utf16_offset = Self::byte_offset_to_utf16_offset(
            &file_content,
            file_relative_byte_offset_u32 as usize,
        );

        let composite_type_string = cm
            .span_to_snippet(type_ann_swc.type_ann.span())
            .unwrap_or_else(|_| "UnknownType".to_string());

        Some(TypeReference {
            file_path: func_def_file_path.to_path_buf(), // Use the function's file path
            type_ann: Some(Box::new(*type_ann_swc.type_ann.clone())), // Store the SWC AST node
            start_position: utf16_offset,
            composite_type_string,
            alias,
        })
    }

    pub fn resolve_types_for_endpoints(&mut self, cm: Lrc<SourceMap>) -> &mut Self {
        let mut request_types_map = HashMap::new();
        let mut response_types_map = HashMap::new();
        let mut seen = HashSet::new();

        // Routers that are mounted on routers can cause duplicate endpoints
        // Lets fix this through dedupe rather than editing the mounting
        self.endpoints.retain(|endpoint| {
            let key = (endpoint.key.clone(), endpoint.handler_name.clone());
            // returns true or false if the value in the set already exists
            seen.insert(key)
        });

        for endpoint in &self.endpoints {
            let Some((method, route)) = endpoint.key.as_http() else {
                continue;
            };
            if let Some(handler_name) = &endpoint.handler_name
                && let Some(func_def) = self.function_definitions.get(handler_name)
                && func_def.arguments.len() >= 2
            {
                // Process Request Type (argument 0)
                if let Some(req_type_ann_swc) = &func_def.arguments[0].type_ann {
                    let alias = Self::generate_common_type_alias_name(
                        route, method, true,  // is_request_type
                        false, // is_consumer = false (endpoints are producers)
                    );
                    if let Some(type_ref) = Self::create_type_reference_from_swc(
                        req_type_ann_swc,
                        &cm,
                        &func_def.file_path,
                        alias,
                    ) {
                        request_types_map.insert(endpoint.key.clone(), type_ref);
                    }
                }

                // Process Response Type (argument 1)
                if let Some(res_type_ann_swc) = &func_def.arguments[1].type_ann {
                    let alias = Self::generate_common_type_alias_name(
                        route, method, false, // is_request_type = false
                        false, // is_consumer = false (endpoints are producers)
                    );
                    if let Some(type_ref) = Self::create_type_reference_from_swc(
                        res_type_ann_swc,
                        &cm,
                        &func_def.file_path,
                        alias,
                    ) {
                        response_types_map.insert(endpoint.key.clone(), type_ref);
                    }
                }
            }
        }

        // Update all endpoints with the resolved types
        for endpoint in &mut self.endpoints {
            if let Some(req_type) = request_types_map.get(&endpoint.key) {
                endpoint.request_type = Some(req_type.clone());
            }
            if let Some(resp_type) = response_types_map.get(&endpoint.key) {
                endpoint.response_type = Some(resp_type.clone());
            }
        }
        self
    }

    // This function analyzes the function definitions and returns a HashMap of route fields.
    pub fn resolve_imported_handler_route_fields(
        &self,
        imported_handlers: &[(String, String, String, String)],
        function_definitions: &HashMap<String, FunctionDefinition>,
    ) -> (RouteFieldMap, RouteFieldMap) {
        let mut response_fields = HashMap::new();
        let mut request_fields = HashMap::new();

        for (route, method, handler_name, _) in imported_handlers {
            if let Some(func_def) = function_definitions.get(handler_name) {
                // Extract response fields from the handler function
                let resp_json = match &func_def.node_type {
                    FunctionNodeType::ArrowFunction(arrow) => self.extract_fields_from_arrow(arrow),
                    FunctionNodeType::FunctionDeclaration(decl) => {
                        self.extract_fields_from_function_decl(decl)
                    }
                    FunctionNodeType::FunctionExpression(expr) => {
                        self.extract_fields_from_function_expr(expr)
                    }
                    FunctionNodeType::Placeholder => {
                        // In CI mode, AST is not available, skip field extraction
                        Json::Null
                    }
                };

                // Extract request body fields from the handler function
                let req_json = match &func_def.node_type {
                    FunctionNodeType::ArrowFunction(arrow) => {
                        if let swc_ecma_ast::BlockStmtOrExpr::BlockStmt(block) = &*arrow.body {
                            self.extract_req_body_fields(block)
                        } else {
                            None
                        }
                    }
                    FunctionNodeType::FunctionDeclaration(decl) => {
                        if let Some(body) = &decl.function.body {
                            self.extract_req_body_fields(body)
                        } else {
                            None
                        }
                    }
                    FunctionNodeType::FunctionExpression(expr) => {
                        if let Some(body) = &expr.function.body {
                            self.extract_req_body_fields(body)
                        } else {
                            None
                        }
                    }
                    FunctionNodeType::Placeholder => {
                        // In CI mode, AST is not available, skip request body extraction
                        None
                    }
                };

                // Store with composite key
                let key = OperationKey::http(method, route.clone());
                response_fields.insert(key.clone(), resp_json);
                if let Some(req) = req_json {
                    request_fields.insert(key, req);
                }
            }
        }

        (response_fields, request_fields)
    }

    // We know endpoints will exist for each imported handler
    pub fn update_endpoints_with_resolved_fields(
        &mut self,
        response_fields: RouteFieldMap,
        request_fields: RouteFieldMap,
    ) -> &mut Self {
        for endpoint in &mut self.endpoints {
            if let Some(response) = response_fields.get(&endpoint.key) {
                endpoint.response_body = Some(response.clone());
            }
            if let Some(request) = request_fields.get(&endpoint.key) {
                endpoint.request_body = Some(request.clone());
            }
        }

        self
    }

    /// Framework-agnostic analysis using mount graph.
    /// Returns `(call_issues, endpoint_issues, env_var_calls, verified_endpoints,
    /// cross_repo_matches)` — the fourth element captures (method, path) of every
    /// endpoint that at least one consumer call successfully matched (positive
    /// signal for the PR comment), and the fifth captures the structured
    /// producer→consumer edges (consumed only by the eval projection).
    fn analyze_matches_with_mount_graph(&self, mount_graph: &MountGraph) -> MountGraphMatches {
        let mut call_issues = Vec::new();
        let mut endpoint_issues = Vec::new();
        let mut env_var_calls = Vec::new();
        // Structured producer→consumer edges for the eval projection.
        let mut cross_repo_matches: Vec<CrossRepoMatch> = Vec::new();

        // Consumer repo lookup: a call's `(METHOD, target_url, file_location)`
        // → owning repo. `merge_from_repos` tags each merged data call with its
        // repo; `self.calls` carry only `(key, file_path)`, so this re-attaches
        // the repo identity at the matching site. Keyed on the full triple
        // because two calls in one file can share a target.
        let consumer_repo_by_call: HashMap<(String, String, String), String> = mount_graph
            .get_data_calls()
            .iter()
            .filter_map(|c| {
                c.repo_name.as_ref().map(|repo| {
                    (
                        (
                            c.method.to_uppercase(),
                            c.target_url.clone(),
                            c.file_location.clone(),
                        ),
                        repo.clone(),
                    )
                })
            })
            .collect();

        // Track which endpoints have been matched
        let mut matched_endpoints: HashSet<String> = HashSet::new();

        // Deduplicate calls
        let mut unique_calls = Vec::new();
        let mut seen_calls = HashSet::new();
        for call in &self.calls {
            // Drop HTTP calls whose target is not a real outgoing-call shape.
            // The file-analyzer LLM sometimes emits SDK ops, bare identifiers,
            // or member expressions as a call target (e.g. `DynamoDB:PutItem`,
            // `res.json()`); those never match a producer and would otherwise
            // flood the report as "missing endpoints" / env-var suggestions.
            // Non-HTTP operations (GraphQL/Socket) are keyed exactly and handled
            // by their own matchers, so this route-shape gate only applies to HTTP.
            if let Some((_, target)) = call.key.as_http()
                && !is_valid_route_shape(target)
            {
                debug!("Skipping call with non-route value: {}", call.key);
                continue;
            }
            let key = format!("{}:{}", call.key.canonical(), call.file_path.display());
            if seen_calls.insert(key) {
                unique_calls.push(call);
            }
        }

        // Create URL normalizer once for all calls
        let normalizer = UrlNormalizer::new(&self.config);

        // For each call, try to find matching endpoint using mount graph.
        // This is the HTTP matcher: non-HTTP operations are dispatched to
        // their own matchers and skipped here.
        for call in &unique_calls {
            let Some((method, target)) = call.key.as_http() else {
                continue;
            };
            // Check for environment variable URLs (framework-agnostic)
            // Use smarter detection to avoid false positives on path parameters
            if Self::is_env_var_base_url(target) {
                let env_var_name = Self::extract_env_var_name(target);
                let normalized_path = normalizer.extract_path(target);
                let canonical_env_var_route =
                    format!("ENV_VAR:{}:{}", env_var_name, normalized_path);

                if self.config.is_external_call(&canonical_env_var_route) {
                    continue;
                }

                if self.config.is_internal_call(&canonical_env_var_route) {
                    match mount_graph.find_matching_endpoints_with_normalizer(
                        &canonical_env_var_route,
                        method,
                        &normalizer,
                    ) {
                        Some(matching_endpoints) => {
                            if matching_endpoints.is_empty() {
                                call_issues.push(format!(
                                    "Missing endpoint for {} {} (normalized: {}) (called from {})",
                                    method,
                                    target,
                                    normalized_path,
                                    call.file_path.display()
                                ));
                            } else {
                                for endpoint in matching_endpoints {
                                    let key = format!("{}:{}", endpoint.method, endpoint.full_path);
                                    matched_endpoints.insert(key);
                                    if let Some(edge) = Self::build_cross_repo_match(
                                        call,
                                        method,
                                        target,
                                        &normalized_path,
                                        endpoint,
                                        &consumer_repo_by_call,
                                    ) {
                                        cross_repo_matches.push(edge);
                                    }
                                }
                            }
                        }
                        None => {
                            // Identified as external - skip
                        }
                    }
                    continue;
                }

                env_var_calls.push(format!(
                    "Unclassified env var: {} {} using [{}] (from {}) - add to internalEnvVars or externalEnvVars in carrick.json",
                    method,
                    normalized_path,
                    env_var_name,
                    call.file_path.display()
                ));
                continue;
            }

            // Use mount graph to find matching endpoints with URL normalization
            // This handles full URLs, env var patterns, template literals, etc.
            match mount_graph.find_matching_endpoints_with_normalizer(target, method, &normalizer) {
                None => {
                    // URL was identified as external - skip it
                    continue;
                }
                Some(matching_endpoints) => {
                    if matching_endpoints.is_empty() {
                        // Extract normalized path for better error message
                        let normalized_path = normalizer.extract_path(target);
                        call_issues.push(format!(
                            "Missing endpoint for {} {} (normalized: {}) (called from {})",
                            method,
                            target,
                            normalized_path,
                            call.file_path.display()
                        ));
                    } else {
                        // Mark endpoints as matched
                        let normalized_path = normalizer.normalize(target).path;
                        for endpoint in matching_endpoints {
                            let key = format!("{}:{}", endpoint.method, endpoint.full_path);
                            matched_endpoints.insert(key);
                            if let Some(edge) = Self::build_cross_repo_match(
                                call,
                                method,
                                target,
                                &normalized_path,
                                endpoint,
                                &consumer_repo_by_call,
                            ) {
                                cross_repo_matches.push(edge);
                            }
                        }
                    }
                }
            }
        }

        // Find orphaned endpoints (not matched by any call), and capture
        // verified matches as (method, path) tuples for the formatter.
        let mut verified: Vec<(String, String)> = Vec::new();
        for endpoint in mount_graph.get_resolved_endpoints() {
            let key = format!("{}:{}", endpoint.method, endpoint.full_path);
            if matched_endpoints.contains(&key) {
                verified.push((endpoint.method.clone(), endpoint.full_path.clone()));
            } else {
                endpoint_issues.push(OrphanedEndpoint {
                    method: endpoint.method.clone(),
                    path: endpoint.full_path.clone(),
                    // Prefer the monorepo service name, falling back to the repo
                    // (matches the cloud's service_name ?? repo_name convention).
                    service: endpoint
                        .service_name
                        .clone()
                        .or_else(|| endpoint.repo_name.clone()),
                });
            }
        }
        verified.sort();
        verified.dedup();

        // Deterministic order for the projection (mirrors verified.sort()): the
        // matcher iterates calls/endpoints in a non-deterministic order, so sort
        // and dedup the captured edges on their identity tuple. The non-HTTP
        // edges added later in `get_results` are re-sorted there over the
        // combined set, so this is the HTTP-only first pass.
        sort_dedup_cross_repo_matches(&mut cross_repo_matches);

        (
            call_issues,
            endpoint_issues,
            env_var_calls,
            verified,
            cross_repo_matches,
        )
    }

    /// Build a [`CrossRepoMatch`] from a matched consumer call + producer
    /// endpoint. Returns `None` only when the consumer's repo cannot be
    /// attributed (no `repo_name` tag in the merged graph for this call) — an
    /// edge without both repo ids is not useful to the scorer.
    ///
    /// `match_score` is `1.0`: every edge captured here is an exact
    /// normalized-key match (there is no finer scorer yet). `type_compatible`
    /// is left `None` here; `get_results` overlays the per-endpoint compat
    /// verdict after type checking has (or has not) run.
    fn build_cross_repo_match(
        call: &ApiEndpointDetails,
        method: &str,
        target: &str,
        normalized_consumer_path: &str,
        endpoint: &crate::mount_graph::ResolvedEndpoint,
        consumer_repo_by_call: &HashMap<(String, String, String), String>,
    ) -> Option<CrossRepoMatch> {
        let producer_repo = endpoint
            .service_name
            .clone()
            .or_else(|| endpoint.repo_name.clone())?;
        let lookup_key = (
            method.to_uppercase(),
            target.to_string(),
            call.file_path.display().to_string(),
        );
        let consumer_repo = consumer_repo_by_call.get(&lookup_key).cloned()?;

        let producer_key = OperationKey::http(&endpoint.method, endpoint.full_path.clone());
        let consumer_key = OperationKey::http(method, normalized_consumer_path.to_string());

        Some(CrossRepoMatch {
            producer_repo,
            producer_key: producer_key.canonical(),
            consumer_repo,
            consumer_key: consumer_key.canonical(),
            // The consumer call's source location — the per-pair join key for the
            // compat verdict (#260). Shares the consumer manifest entry's source
            // (both come from this call's `file_location`), so the overlay can
            // attribute ts_check's `consumerLocation` to THIS edge.
            consumer_location: Some(call.file_path.display().to_string()),
            match_score: 1.0,
            type_compatible: None,
            mismatch_reason: None,
        })
    }

    /// Match consumers against producers of a protocol whose operations have
    /// exact key identity (GraphQL fields, socket events) — no URL or mount
    /// hierarchy to normalize. Returns `(call_issues, endpoint_issues,
    /// verified)`.
    ///
    /// If no producer of the protocol is indexed anywhere, consumers are
    /// skipped silently: the producing service may simply not be scanned,
    /// and guessing would create false "missing endpoint" noise. Unconsumed
    /// producers are reported as orphans, the same soft signal REST orphans
    /// get.
    fn analyze_exact_key_matches(
        &self,
        protocol: crate::operation::Protocol,
        protocol_label: &str,
    ) -> ExactKeyMatches {
        let producer_keys: HashSet<&OperationKey> = self
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.key.protocol() == protocol)
            .map(|endpoint| &endpoint.key)
            .collect();
        if producer_keys.is_empty() {
            return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        }

        // Producer repo ids (service_name ?? repo_name) per canonical key, so a
        // matched consumer can be attributed for a `CrossRepoMatch`. A key with
        // no repo identity yields no edge (the same guard the HTTP path applies).
        // Multiple producers can legitimately share one exact key — two services
        // exposing the same GraphQL field, or several listeners for one socket
        // event — and exact-key matching has no URL to disambiguate them. So
        // collect ALL distinct producer repos (a `BTreeSet` for deterministic
        // order) and emit one edge per producer↔consumer pair, rather than
        // arbitrarily keeping the first by iteration order.
        let mut producer_repos_by_key: HashMap<String, std::collections::BTreeSet<String>> =
            HashMap::new();
        for endpoint in &self.endpoints {
            if endpoint.key.protocol() != protocol {
                continue;
            }
            if let Some(repo) = endpoint
                .service_name
                .clone()
                .or_else(|| endpoint.repo_name.clone())
            {
                producer_repos_by_key
                    .entry(endpoint.key.canonical())
                    .or_default()
                    .insert(repo);
            }
        }

        let mut call_issues = Vec::new();
        let mut cross_repo_matches: Vec<CrossRepoMatch> = Vec::new();
        let mut matched: HashSet<&OperationKey> = HashSet::new();
        let mut seen_calls = HashSet::new();
        for call in &self.calls {
            if call.key.protocol() != protocol {
                continue;
            }
            let dedup = format!("{}:{}", call.key.canonical(), call.file_path.display());
            if !seen_calls.insert(dedup) {
                continue;
            }
            if producer_keys.contains(&call.key) {
                matched.insert(&call.key);
                // Emit the cross-repo edge. Exact-key protocols share one key on
                // both sides, so producer_key == consumer_key. For sockets the
                // producer is the listener (an endpoint) and the consumer is the
                // emitter (a call); this attribution follows directly from which
                // side the op sits on. `type_compatible` is left `None` —
                // `overlay_compat_verdicts` fills it in if compat ran.
                let consumer_repo = call.service_name.clone().or_else(|| call.repo_name.clone());
                let canonical = call.key.canonical();
                if let (Some(producer_repos), Some(consumer_repo)) =
                    (producer_repos_by_key.get(&canonical), consumer_repo)
                {
                    for producer_repo in producer_repos {
                        cross_repo_matches.push(CrossRepoMatch {
                            producer_repo: producer_repo.clone(),
                            producer_key: canonical.clone(),
                            consumer_repo: consumer_repo.clone(),
                            consumer_key: canonical.clone(),
                            // Exact-key protocols (GraphQL/socket) ARE type-checked
                            // by ts_check now, so this consumer location feeds the
                            // compat overlay (`apply_compat_verdicts`) and also keeps
                            // the dedup identity precise.
                            consumer_location: Some(call.file_path.display().to_string()),
                            match_score: 1.0,
                            type_compatible: None,
                            mismatch_reason: None,
                        });
                    }
                }
            } else {
                let (label, name) = call.key.display_labels();
                call_issues.push(format!(
                    "Missing endpoint for {} {} ({}; called from {})",
                    label,
                    name,
                    protocol_label,
                    call.file_path.display()
                ));
            }
        }

        let mut endpoint_issues = Vec::new();
        let mut verified = Vec::new();
        let mut seen_producers = HashSet::new();
        for endpoint in &self.endpoints {
            if endpoint.key.protocol() != protocol {
                continue;
            }
            if !seen_producers.insert(endpoint.key.canonical()) {
                continue;
            }
            let (label, name) = endpoint.key.display_labels();
            if matched.contains(&endpoint.key) {
                verified.push((label, name));
            } else {
                // GraphQL/socket producers are not repo-tagged at this layer, so
                // the owning service is unknown.
                endpoint_issues.push(OrphanedEndpoint {
                    method: label,
                    path: name,
                    service: None,
                });
            }
        }
        verified.sort();
        verified.dedup();

        (call_issues, endpoint_issues, verified, cross_repo_matches)
    }

    pub fn compute_full_paths_for_endpoint(
        endpoint: &ApiEndpointDetails,
        mounts: &[Mount],
        _apps: &std::collections::HashMap<String, AppContext>,
    ) -> Vec<String> {
        let mut results = Vec::new();

        // Defensive: skip endpoints with no owner
        let mut owner = match &endpoint.owner {
            Some(owner) => owner.clone(),
            None => return results,
        };

        // Mount-prefix resolution only applies to HTTP routes
        let Some((_, route)) = endpoint.key.as_http() else {
            return results;
        };
        let mut path = route.to_string();
        let mut visited = std::collections::HashSet::new();

        // Walk up the mount chain, prepending prefixes
        loop {
            // Prevent cycles
            if !visited.insert(owner.clone()) {
                break;
            }

            // Find the mount where this owner is the child
            if let Some(mount) = mounts.iter().find(|m| m.child == owner) {
                // Prepend the prefix
                path = join_prefix_and_path(&mount.prefix, &path);
                // Move up to the parent
                owner = mount.parent.clone();
                // If the parent is an app, we're done
                if let OwnerType::App(_) = owner {
                    results.push(path.clone());
                    break;
                }
            } else {
                // If owner is an app, just push the path
                if let OwnerType::App(_) = owner {
                    results.push(path.clone());
                }
                // No more parents, stop
                break;
            }
        }

        results
    }

    pub fn resolve_all_endpoint_paths(
        &self,
        endpoints: &[ApiEndpointDetails],
        mounts: &[Mount],
        apps: &std::collections::HashMap<String, AppContext>,
    ) -> Vec<ApiEndpointDetails> {
        let mut new_endpoints = Vec::new();
        for endpoint in endpoints {
            let Some((method, _)) = endpoint.key.as_http() else {
                new_endpoints.push(endpoint.clone());
                continue;
            };
            let method = method.to_string();
            let full_paths = Self::compute_full_paths_for_endpoint(endpoint, mounts, apps);
            for path in full_paths {
                let mut ep = endpoint.clone();
                ep.key = OperationKey::http(&method, path);
                new_endpoints.push(ep);
            }
        }
        new_endpoints
    }

    fn normalize_route_params(&self, route: &str) -> String {
        // Replace all parameter placeholders with a consistent name.
        ROUTE_PARAM_RE.replace_all(route, "{param}").to_string()
    }

    pub fn build_endpoint_router(&mut self) {
        let mut router = matchit::Router::new();

        // Use a HashMap to collect all endpoints by path before inserting into router
        let mut path_to_endpoints: HashMap<String, Vec<(String, String)>> = HashMap::new();

        for endpoint in &self.endpoints {
            let Some((method, route)) = endpoint.key.as_http() else {
                continue;
            };
            let normalized_route = self.normalize_route_params(route);

            path_to_endpoints
                .entry(normalized_route)
                .or_default()
                .push((route.to_string(), method.to_string()));
        }

        debug!("Unique endpoint paths: {}", path_to_endpoints.len());

        // Now insert each unique path once, with a collection of route-method pairs
        for (path, route_methods) in path_to_endpoints {
            if let Err(e) = router.insert(&path, route_methods) {
                warn!("Could not add route to router: {}", e);
            }
        }

        self.endpoint_router = Some(router);
    }

    pub fn check_type_compatibility(&self) -> Result<serde_json::Value, String> {
        use std::fs;

        let output_dir = self.ts_check_output_dir().ok_or_else(|| {
            "ts_check/ directory was not discovered. Ensure the carrick install \
             includes ts_check/ adjacent to the binary."
                .to_string()
        })?;

        // Ensure the output directory exists
        if !output_dir.exists() {
            return Err(format!(
                "Output directory {} does not exist",
                output_dir.display()
            ));
        }

        // Check for type-check-results.json file created by the integrated type checker
        let results_file = output_dir.join("type-check-results.json");

        if !results_file.exists() {
            return Err("Type check results file not found. Type checking may have failed during extraction.".to_string());
        }

        // Read the type check results
        let contents = fs::read_to_string(results_file)
            .map_err(|e| format!("Failed to read type check results: {}", e))?;

        // Parse the JSON output
        let result: serde_json::Value = serde_json::from_str(&contents).map_err(|e| {
            format!(
                "Failed to parse type checking result: {}. Raw content: '{}'",
                e, contents
            )
        })?;

        // Check for error in the result
        if let Some(error) = result.get("error") {
            return Err(format!("Type checking failed: {}", error));
        }

        // Transform result to match expected format
        self.transform_type_check_result(result)
    }

    fn transform_type_check_result(
        &self,
        result: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let mismatches = result.get("mismatches")
            .and_then(|m| m.as_array())
            .unwrap_or(&vec![])
            .iter()
            .map(|mismatch| {
                // `consumerLocation` is the per-consumer join key the overlay
                // needs to attribute this verdict to a single (producer, consumer)
                // edge rather than smearing it across all consumers (#260) — it
                // must survive this transform.
                serde_json::json!({
                    "endpoint": mismatch.get("endpoint").unwrap_or(&serde_json::Value::Null),
                    "producerType": mismatch.get("producerType").unwrap_or(&serde_json::Value::Null),
                    "consumerType": mismatch.get("consumerType").unwrap_or(&serde_json::Value::Null),
                    "producerLocation": mismatch.get("producerLocation").unwrap_or(&serde_json::Value::Null),
                    "consumerLocation": mismatch.get("consumerLocation").unwrap_or(&serde_json::Value::Null),
                    "error": mismatch.get("error").unwrap_or(&serde_json::Value::Null)
                })
            })
            .collect::<Vec<_>>();

        // Edges ts_check matched but could not verify — a side resolved to
        // `any`/`unknown` (e.g. the type never reached the bundled .d.ts). These
        // are NOT compatible; they are unverifiable, and the overlay must leave
        // their verdict `None` rather than optimistically claiming `Some(true)`.
        let unknown_pairs = result
            .get("unknownPairs")
            .and_then(|u| u.as_array())
            .unwrap_or(&vec![])
            .iter()
            .map(|pair| {
                // Carry `consumerLocation` for the same per-consumer keying (#260).
                serde_json::json!({
                    "endpoint": pair.get("endpoint").unwrap_or(&serde_json::Value::Null),
                    "producerLocation": pair.get("producerLocation").unwrap_or(&serde_json::Value::Null),
                    "consumerLocation": pair.get("consumerLocation").unwrap_or(&serde_json::Value::Null),
                    "reason": pair.get("reason").unwrap_or(&serde_json::Value::Null)
                })
            })
            .collect::<Vec<_>>();

        Ok(serde_json::json!({
            "mismatches": mismatches,
            "unknownPairs": unknown_pairs,
            "totalChecked": result.get("totalChecked").unwrap_or(&serde_json::Value::Number(serde_json::Number::from(0))),
            "compatiblePairs": result.get("compatibleCount").unwrap_or(&serde_json::Value::Number(serde_json::Number::from(0))),
            "incompatiblePairs": mismatches.len()
        }))
    }

    pub fn get_results(&self) -> ApiAnalysisResult {
        // Framework-agnostic analysis using mount graph (required)
        let mount_graph = self.mount_graph.as_ref()
            .expect("Mount graph must be set before calling get_results(). This is a framework-agnostic requirement.");

        let (
            mut call_issues,
            mut endpoint_issues,
            env_var_calls,
            mut verified_endpoints,
            mut cross_repo_matches,
        ) = self.analyze_matches_with_mount_graph(mount_graph);
        for (protocol, label) in [
            (crate::operation::Protocol::Graphql, "GraphQL"),
            (crate::operation::Protocol::Websocket, "Socket.IO"),
        ] {
            let (
                protocol_call_issues,
                protocol_endpoint_issues,
                protocol_verified,
                protocol_cross_repo_matches,
            ) = self.analyze_exact_key_matches(protocol, label);
            call_issues.extend(protocol_call_issues);
            endpoint_issues.extend(protocol_endpoint_issues);
            verified_endpoints.extend(protocol_verified);
            cross_repo_matches.extend(protocol_cross_repo_matches);
        }
        verified_endpoints.sort();
        verified_endpoints.dedup();
        // Re-sort/dedup over the combined HTTP + non-HTTP edge set so the final
        // ordering is stable regardless of which matcher produced an edge.
        sort_dedup_cross_repo_matches(&mut cross_repo_matches);
        // Note: JSON body comparison removed - type checking is done via TypeScript (ts_check/)
        let mismatches = Vec::new();
        let type_mismatches = self.get_type_mismatches();
        let dependency_conflicts = self.analyze_dependencies();

        // Overlay the per-pair type-compat verdict onto the captured edges. The
        // verdict is keyed by the producer's (METHOD, full_path) AND the consumer's
        // source location, so each (producer, consumer) edge gets its own verdict
        // rather than sharing the producer's first verdict across all consumers
        // (#260).
        //
        // `type_compatible` stays `None` when compat was not evaluated
        // (`check_type_compatibility` returns `Err`: ts_check_dir absent, results
        // file missing, or type checking failed). This `None` is load-bearing:
        // the scorer must never read absent compat data as "compatible".
        self.overlay_compat_verdicts(&mut cross_repo_matches);

        let detected_graphql_libraries = filter_graphql_libraries(&self.detected_data_fetchers);
        let graphql_operations_indexed = self
            .endpoints
            .iter()
            .chain(self.calls.iter())
            .any(|details| details.key.protocol() == crate::operation::Protocol::Graphql);

        // Canonical ordering: the analyzer collects endpoints/calls in a
        // non-deterministic order (HashMap iteration + concurrent file joins
        // upstream), so two scans of the same repo can emit the same set in a
        // different sequence. Sort here, at the single aggregation point, so
        // *every* consumer (PR comment, dashboard upload, eval projection, the
        // cassette hard gate) sees a stable order. Keyed on the canonical
        // operation key then the `<file>:<line>` location to fully disambiguate
        // same-key operations. Mirrors the adjacent `verified_endpoints.sort()`.
        // The key allocates (canonical() + owned path string), so use
        // sort_by_cached_key: it computes each element's key once, not once per
        // comparison.
        let sort_key = |d: &ApiEndpointDetails| {
            (
                d.key.canonical(),
                d.file_path.to_string_lossy().into_owned(),
            )
        };
        let mut endpoints = self.endpoints.clone();
        let mut calls = self.calls.clone();
        endpoints.sort_by_cached_key(&sort_key);
        calls.sort_by_cached_key(&sort_key);

        ApiAnalysisResult {
            endpoints,
            calls,
            issues: ApiIssues {
                call_issues,
                endpoint_issues,
                env_var_calls,
                mismatches,
                type_mismatches,
                dependency_conflicts,
            },
            verified_endpoints,
            detected_graphql_libraries,
            graphql_operations_indexed,
            cross_repo_matches,
        }
    }

    /// Overlay the type-compatibility verdict onto each captured cross-repo
    /// edge, keyed by the producer's `(METHOD, full_path)` AND the consumer's
    /// source location — so each `(producer, consumer)` pair gets ITS OWN
    /// verdict.
    ///
    /// ts_check emits one mismatch/unknownPair entry per matched producer↔
    /// consumer pair, each carrying `consumerLocation` (`"<file>:<line>"`). Keying
    /// only on the producer `(METHOD, path)` collapsed all consumers of a producer
    /// into one verdict: when one producer had ≥2 consumers of differing
    /// compatibility, the first verdict smeared onto every edge (#260 — the
    /// flagship false-negative). The key now includes the consumer identity
    /// (`parse_file_location(consumerLocation)`), which the edge mirrors in
    /// `consumer_location` (both derive from the same call `file_location`).
    ///
    /// If `check_type_compatibility` returns `Err`, type checking did not run
    /// (or failed) for this scan: every edge keeps `type_compatible: None`
    /// (load-bearing — see [`CrossRepoMatch`]). On `Ok`, an edge whose
    /// `(producer, consumer)` pair appears in the mismatch set gets `Some(false)`
    /// with the reason. A pair ts_check matched but could NOT verify (a side
    /// resolved to `any`/`unknown`, e.g. the type never reached the bundled
    /// `.d.ts`) keeps `None` — unverifiable, not compatible. Everything else
    /// (genuinely checked and compatible) gets `Some(true)`.
    fn overlay_compat_verdicts(&self, matches: &mut [CrossRepoMatch]) {
        let result = match self.check_type_compatibility() {
            Ok(result) => result,
            // Compat was not evaluated for this run — leave every edge `None`.
            Err(_) => return,
        };
        apply_compat_verdicts(&result, matches);
    }

    pub fn run_final_type_checking(&self) -> Result<(), String> {
        use std::fs;
        use std::process::Command;

        // Resolve the ts_check/ directory (discovered at CLI entry time).
        let ts_check_dir = self.ts_check_dir.as_ref().ok_or_else(|| {
            "ts_check/ directory was not discovered. The carrick binary could not \
             locate ts_check/run-type-checking.ts adjacent to itself. Expected \
             layouts: <exe_dir>/ts_check, <exe_dir>/../ts_check, or \
             <exe_dir>/../lib/ts_check. This usually means the install is incomplete."
                .to_string()
        })?;

        let script_path = ts_check_dir.join("run-type-checking.ts");
        if !script_path.exists() {
            return Err(format!(
                "Type checking script not found at {}. Expected a complete ts_check/ \
                 directory adjacent to the carrick binary.",
                script_path.display()
            ));
        }

        // Create minimal tsconfig.json in output directory
        let output_dir = ts_check_dir.join("output");
        fs::create_dir_all(&output_dir)
            .map_err(|e| format!("Failed to create output directory: {}", e))?;

        // Check if there are any bundled .d.ts files to check
        let type_files: Vec<_> = fs::read_dir(&output_dir)
            .map_err(|e| format!("Failed to read output directory: {}", e))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .path()
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".d.ts"))
            })
            .collect();

        if type_files.is_empty() {
            debug!(
                "No bundled .d.ts files found in {} - skipping type checking",
                output_dir.display()
            );
            debug!("   This may happen if:");
            debug!("   - Source code lacks explicit TypeScript type annotations");
            debug!("   - Type extraction agents couldn't identify response/request types");
            debug!("   - This is the first run and no cross-repo data exists yet");
            debug!("   Type checking will work when type annotations are present in the source.");
            return Ok(());
        }

        debug!(
            "Found {} type file(s) to check: {:?}",
            type_files.len(),
            type_files.iter().map(|f| f.file_name()).collect::<Vec<_>>()
        );

        let tsconfig_path = output_dir.join("tsconfig.json");
        let tsconfig_content = create_standard_tsconfig();

        fs::write(
            &tsconfig_path,
            serde_json::to_string_pretty(&tsconfig_content).unwrap(),
        )
        .map_err(|e| format!("Failed to create tsconfig.json: {}", e))?;

        let producer_manifest = output_dir.join("producer-manifest.json");
        let consumer_manifest = output_dir.join("consumer-manifest.json");

        if !producer_manifest.exists() || !consumer_manifest.exists() {
            return Err(format!(
                "Producer/consumer manifest files not found in {}",
                output_dir.display()
            ));
        }

        // Run the type checking script with the minimal tsconfig.
        //
        // - `current_dir(ts_check_dir)`: resolve `npx ts-node` against
        //   `ts_check/node_modules/.bin` instead of letting npx download a
        //   transient ts-node that can't see ts-morph / @types/node and fails to
        //   compile (the #226 root cause: ts_check deps weren't installed, so the
        //   checker never ran and every verdict stayed None).
        // - `-o <output_dir>` (absolute): the script otherwise defaults its
        //   output to the CWD-relative `ts_check/output`, which need not coincide
        //   with the discovered, absolute output dir this analyzer reads back from
        //   in `check_type_compatibility`. Pin it so the results file always lands
        //   where the verdict overlay looks for it.
        let output = Command::new("npx")
            .current_dir(ts_check_dir)
            .arg("ts-node")
            .arg(&script_path)
            .arg(&tsconfig_path)
            .arg("--producer")
            .arg(&producer_manifest)
            .arg("--consumer")
            .arg(&consumer_manifest)
            .arg("--types-dir")
            .arg(&output_dir)
            .arg("-o")
            .arg(&output_dir)
            .output()
            .map_err(|e| format!("Failed to run type checking: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Log type checking output at debug level
        if !stdout.trim().is_empty() {
            for line in stdout.lines() {
                debug!("{}", line);
            }
        }
        if !stderr.trim().is_empty() && !output.status.success() {
            for line in stderr.lines() {
                debug!("{}", line);
            }
        }

        if !output.status.success() {
            // Surface a stderr tail in the error: a bare exit code hides the
            // common failure mode (ts-node can't resolve ts_check deps), which is
            // exactly what made #226 opaque. The caller only `warn!`s this, so the
            // detail must travel with the message.
            let tail: String = stderr.lines().rev().take(8).collect::<Vec<_>>().join(" | ");
            return Err(format!(
                "Type checking script failed with exit code: {:?}. stderr tail: {}",
                output.status.code(),
                tail
            ));
        }

        Ok(())
    }

    fn build_display_name_map(&self) -> HashMap<String, String> {
        use std::fs;

        let mut map = HashMap::new();
        let Some(output_dir) = self.ts_check_output_dir() else {
            return map;
        };

        for manifest_path in &[
            output_dir.join("producer-manifest.json"),
            output_dir.join("consumer-manifest.json"),
        ] {
            let Ok(contents) = fs::read_to_string(manifest_path) else {
                continue;
            };
            let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&contents) else {
                continue;
            };
            let Some(entries) = parsed.get("entries").and_then(|e| e.as_array()) else {
                continue;
            };
            for entry in entries {
                let Ok(entry) = serde_json::from_value::<crate::cloud_storage::TypeManifestEntry>(
                    entry.clone(),
                ) else {
                    continue;
                };
                let type_kind = match entry.type_kind {
                    crate::cloud_storage::ManifestTypeKind::Request => "request",
                    crate::cloud_storage::ManifestTypeKind::Response => "response",
                };
                let display = crate::type_manifest::build_display_name(&entry.key, type_kind);
                map.insert(entry.type_alias.clone(), display);
            }
        }

        map
    }

    fn get_type_mismatches(&self) -> Vec<String> {
        match self.check_type_compatibility() {
            Ok(result) => {
                let display_names = self.build_display_name_map();

                if let Some(mismatches) = result.get("mismatches").and_then(|m| m.as_array()) {
                    mismatches.iter()
                        .filter_map(|mismatch| {
                            if let (Some(endpoint), Some(producer), Some(consumer), Some(error)) = (
                                mismatch.get("endpoint").and_then(|e| e.as_str()),
                                mismatch.get("producerType").and_then(|t| t.as_str()),
                                mismatch.get("consumerType").and_then(|t| t.as_str()),
                                mismatch.get("error").and_then(|e| e.as_str()),
                            ) {
                                // Clean up import paths for better readability
                                let clean_producer = self.clean_type_string(producer, &display_names);
                                let clean_consumer = self.clean_type_string(consumer, &display_names);
                                let clean_error = self.clean_error_message(error, &display_names);

                                Some(format!(
                                    "Type mismatch on {}: Producer ({}) incompatible with Consumer ({}) - {}",
                                    endpoint,
                                    clean_producer,
                                    clean_consumer,
                                    clean_error
                                ))
                            } else {
                                None
                            }
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            }
            Err(_) => Vec::new(),
        }
    }

    fn clean_type_string(&self, type_str: &str, display_names: &HashMap<String, String>) -> String {
        // Remove absolute paths from import statements, keeping only the relative part
        let mut cleaned = IMPORT_PATH_RE
            .replace_all(type_str, |caps: &regex::Captures| {
                let type_name = &caps[2];
                // Replace hash-based type aliases with display names
                if let Some(display) = display_names.get(type_name) {
                    return display.clone();
                }
                let path = &caps[1];
                // Extract just the filename without path for readability
                if let Some(filename) = path.split('/').next_back() {
                    format!("{}.{}", filename, type_name)
                } else {
                    format!("{}.{}", path, type_name)
                }
            })
            .to_string();

        // Also replace standalone hash-based type aliases (not inside import())
        for (alias, display) in display_names {
            if cleaned.contains(alias.as_str()) {
                cleaned = cleaned.replace(alias.as_str(), display);
            }
        }

        // Simplify Array<T> to T[]
        cleaned = ARRAY_GENERIC_RE.replace_all(&cleaned, "$1[]").to_string();

        cleaned
    }

    fn clean_error_message(&self, error: &str, display_names: &HashMap<String, String>) -> String {
        let mut cleaned = error
            .replace("Type '", "")
            .replace(
                "' is missing the following properties from type '",
                " missing properties from ",
            )
            .replace("': ", ": ")
            .replace("' is not assignable to type '", " not assignable to ")
            .replace("'.", "");

        // Replace hash-based type aliases in error messages
        for (alias, display) in display_names {
            if cleaned.contains(alias.as_str()) {
                cleaned = cleaned.replace(alias.as_str(), display);
            }
        }

        cleaned
    }

    /// Extract repository prefix from endpoint owner information
    /// Note: Currently unused but kept for future multi-repo scenarios where
    /// owner names might contain repo prefixes (format: "repo_prefix:name")
    #[allow(dead_code)]
    pub fn extract_repo_prefix_from_owner(&self, owner: &Option<OwnerType>) -> String {
        if let Some(owner) = owner {
            match owner {
                OwnerType::App(name) | OwnerType::Router(name) => {
                    // Extract repo prefix from owner name (format: "repo_prefix:name")
                    name.split(':').next().unwrap_or("default").to_string()
                }
            }
        } else {
            "default".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-service dependency conflicts are reported only when semver-
    /// INCOMPATIBLE (a major-version spread). `zod` 3.22.0 vs 4.0.0 is a real
    /// conflict (Critical); `typescript` 5.3.0 vs 5.4.0 is a compatible same-major
    /// drift and must be suppressed — it was the false positive that pinned the
    /// xrepo-corpus-1 dependency F1 to 0.667 (precision 0.5). Non-semver pins that
    /// differ as raw strings are reported conservatively.
    #[test]
    fn dependency_conflict_reported_only_when_major_incompatible() {
        fn infos(versions: &[&str]) -> Vec<RepoPackageInfo> {
            versions
                .iter()
                .enumerate()
                .map(|(i, v)| RepoPackageInfo {
                    repo_name: format!("repo-{i}"),
                    version: (*v).to_string(),
                    source_path: PathBuf::from("package.json"),
                })
                .collect()
        }

        // zod 3.x vs 4.x — major spread → reported, Critical.
        let zod = infos(&["3.22.0", "4.0.0"]);
        assert!(Analyzer::is_reportable_conflict(&zod));
        assert!(matches!(
            Analyzer::determine_conflict_severity(&zod),
            ConflictSeverity::Critical
        ));

        // typescript 5.3 vs 5.4 — same-major minor drift → suppressed.
        assert!(!Analyzer::is_reportable_conflict(&infos(&["5.3.0", "5.4.0"])));
        // patch-only drift → also suppressed.
        assert!(!Analyzer::is_reportable_conflict(&infos(&["1.1.1", "1.1.2"])));
        // three same-major versions → suppressed.
        assert!(!Analyzer::is_reportable_conflict(&infos(&[
            "18.2.0", "18.3.0", "18.3.1"
        ])));

        // A non-semver pin that differs as a raw string → reported conservatively
        // (has_conflicts upstream already established the strings differ).
        assert!(Analyzer::is_reportable_conflict(&infos(&[
            "workspace:*",
            "1.0.0"
        ])));
    }

    #[test]
    fn route_shape_drops_non_route_values() {
        // Exact values the file-analyzer LLM mis-emitted as call targets on the
        // carrick-cloud run (SDK ops, bare identifiers, member/call expressions,
        // literals, leaked `||`).
        let dropped = [
            "DynamoDB:PutItem",
            "DynamoDB:Query",
            "DynamoDB.TransactWriteItems",
            "DynamoDBClient",
            "DynamoDB",
            "GetCommand",
            "QueryCommand",
            "new",
            "null",
            ".",
            "unknown",
            "query",
            "request",
            "request.formData()",
            "res.json()",
            ".json()",
            "result.response.text()",
            "ordersResp",
            "listRes",
            "serviceName",
            "params.service",
            "getAllRepoData",
            "search_by_intent",
            "scaffold",
            "CarrickApiKeys",
            "user#${auth.user_id}",
            "${API_KEYS_TABLE}||CarrickApiKeys",
            "",
        ];
        for route in dropped {
            assert!(
                !is_valid_route_shape(route),
                "expected route to be dropped: {route:?}"
            );
        }
    }

    #[test]
    fn route_shape_keeps_real_routes() {
        let kept = [
            "/mcp",
            "/getAllRepoData",
            "/findService",
            "/users/${userId}",
            "/api/orders/:id",
            "${GITHUB_API}/repos/:owner/:repo",
            "${RESEND_ENDPOINT}/",
            "${lambdaUrl}",
            "${process.env.API_BASE}/users",
            "https://api.github.com/repos/owner/repo",
            "http://localhost:3000/health",
        ];
        for route in kept {
            assert!(
                is_valid_route_shape(route),
                "expected route to be kept: {route:?}"
            );
        }
    }

    #[test]
    fn test_filter_graphql_libraries() {
        let data_fetchers = vec![
            "axios".to_string(),
            "graphql-request".to_string(),
            "@apollo/client".to_string(),
            "urql".to_string(),
            "got".to_string(),
            "node-fetch".to_string(),
            "@urql/core".to_string(),
            "relay-runtime".to_string(),
        ];
        let mut found = filter_graphql_libraries(&data_fetchers);
        found.sort();
        assert_eq!(
            found,
            vec![
                "@apollo/client".to_string(),
                "@urql/core".to_string(),
                "graphql-request".to_string(),
                "relay-runtime".to_string(),
                "urql".to_string(),
            ]
        );
    }

    #[test]
    fn test_filter_graphql_libraries_empty_when_rest_only() {
        let data_fetchers = vec!["axios".to_string(), "fetch".to_string(), "got".to_string()];
        let found = filter_graphql_libraries(&data_fetchers);
        assert!(found.is_empty());
    }

    #[test]
    fn test_sanitize_route_colon_params() {
        // Standard :param style path parameters
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/users/:id"),
            "UsersById"
        );
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/users/:userId/comments"),
            "UsersByUseridComments"
        );
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/api/:id/comments/:commentId"),
            "ApiByIdCommentsByCommentid"
        );
    }

    #[test]
    fn test_sanitize_route_template_literal_params() {
        // Template literal ${param} style path parameters
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/users/${userId}"),
            "UsersByUserid"
        );
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/users/${userId}/comments"),
            "UsersByUseridComments"
        );
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/api/${postId}/comments/${commentId}"),
            "ApiByPostidCommentsByCommentid"
        );
    }

    #[test]
    fn test_sanitize_route_template_literal_with_dot_notation() {
        // Template literals with process.env or object property access
        // Should use the last part (the actual variable name)
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/orders/${process.env.ORDER_ID}"),
            "OrdersByOrderId"
        );
    }

    #[test]
    fn test_sanitize_route_mixed_params() {
        // Mix of :param and ${param} styles (unlikely but should work)
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/users/:id/posts/${postId}"),
            "UsersByIdPostsByPostid"
        );
    }

    #[test]
    fn test_sanitize_route_no_params() {
        // Paths without any parameters
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/api/users"),
            "ApiUsers"
        );
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/health"),
            "Health"
        );
    }

    #[test]
    fn test_sanitize_route_root_path() {
        assert_eq!(Analyzer::sanitize_route_for_dynamic_paths("/"), "");
    }

    #[test]
    fn test_sanitize_route_empty_segments() {
        // Should handle double slashes gracefully
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/api//users"),
            "ApiUsers"
        );
    }

    #[test]
    fn test_sanitize_route_strips_query_params() {
        // Query parameters should be stripped before processing
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/orders?userId=123"),
            "Orders"
        );
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/users/:id?include=posts"),
            "UsersById"
        );
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/api/data?page=1&limit=10"),
            "ApiData"
        );
        assert_eq!(
            Analyzer::sanitize_route_for_dynamic_paths("/orders?userId=:userId"),
            "Orders"
        );
    }

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(Analyzer::to_pascal_case("userId"), "Userid");
        assert_eq!(Analyzer::to_pascal_case("user_id"), "UserId");
        assert_eq!(Analyzer::to_pascal_case("user-id"), "UserId");
        assert_eq!(Analyzer::to_pascal_case("USER"), "User");
        assert_eq!(Analyzer::to_pascal_case(""), "");
    }

    #[test]
    fn test_generate_unique_call_alias_name_with_template_params() {
        // Verify the full alias generation works with template literal paths
        let alias = Analyzer::generate_unique_call_alias_name(
            "/users/${userId}/comments",
            "GET",
            false, // is_request_type
            1,     // call_number
            true,  // is_consumer
        );

        assert!(
            alias.contains("ByUserid"),
            "Alias should contain 'ByUserid'. Got: {}",
            alias
        );
        assert!(
            alias.starts_with("Get"),
            "Alias should start with 'Get'. Got: {}",
            alias
        );
        assert!(
            alias.contains("Consumer"),
            "Alias should contain 'Consumer'. Got: {}",
            alias
        );
    }

    #[test]
    fn test_extract_env_var_name() {
        // ENV_VAR:NAME:/path format
        assert_eq!(
            Analyzer::extract_env_var_name("ENV_VAR:API_URL:/users"),
            "API_URL"
        );
        assert_eq!(
            Analyzer::extract_env_var_name("ENV_VAR:ORDER_SERVICE_URL:/orders"),
            "ORDER_SERVICE_URL"
        );

        // ${process.env.VAR} format
        assert_eq!(
            Analyzer::extract_env_var_name("${process.env.SERVICE_URL}/orders"),
            "SERVICE_URL"
        );
        assert_eq!(
            Analyzer::extract_env_var_name("${process.env.API_BASE}/users/123"),
            "API_BASE"
        );

        // ${VAR} format (without process.env)
        assert_eq!(
            Analyzer::extract_env_var_name("${BASE_URL}/orders"),
            "BASE_URL"
        );

        // process.env.VAR without ${}
        assert_eq!(
            Analyzer::extract_env_var_name("process.env.MY_API_URL + \"/data\""),
            "MY_API_URL"
        );

        // Unknown/fallback
        assert_eq!(Analyzer::extract_env_var_name("unknown"), "UNKNOWN_API");
        assert_eq!(Analyzer::extract_env_var_name("/users"), "UNKNOWN_API");
    }

    #[test]
    fn test_is_env_var_base_url() {
        // Should return true for env var base URLs
        assert!(Analyzer::is_env_var_base_url("ENV_VAR:API_URL:/users"));
        assert!(Analyzer::is_env_var_base_url(
            "ENV_VAR:ORDER_SERVICE_URL:/orders"
        ));
        assert!(Analyzer::is_env_var_base_url(
            "${process.env.API_URL}/users"
        ));
        assert!(Analyzer::is_env_var_base_url(
            "${process.env.SERVICE_URL}/orders"
        ));
        assert!(Analyzer::is_env_var_base_url("${API_BASE_URL}/users"));
        assert!(Analyzer::is_env_var_base_url("${ORDER_SERVICE}/orders"));
        assert!(Analyzer::is_env_var_base_url(
            "process.env.API_URL + \"/data\""
        ));

        // Should return false for path parameters (not base URL env vars)
        assert!(!Analyzer::is_env_var_base_url("/users/${userId}"));
        assert!(!Analyzer::is_env_var_base_url("/api/${version}/data"));
        assert!(!Analyzer::is_env_var_base_url("/orders/${orderId}/items"));
        assert!(!Analyzer::is_env_var_base_url("/users/:id"));
        assert!(!Analyzer::is_env_var_base_url("/api/users"));

        // Edge cases
        assert!(!Analyzer::is_env_var_base_url("${userId}")); // lowercase, not env var pattern
        assert!(!Analyzer::is_env_var_base_url("${camelCase}/path")); // camelCase, not env var
        assert!(Analyzer::is_env_var_base_url("${API_V2}/users")); // UPPER_CASE with digit
    }
    fn graphql_details(key: OperationKey, file: &str) -> ApiEndpointDetails {
        ApiEndpointDetails {
            owner: None,
            key,
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: None,
            request_type: None,
            response_type: None,
            file_path: PathBuf::from(file),
            repo_name: None,
            service_name: None,
        }
    }

    /// Like [`graphql_details`] but stamps repo identity (as the cross-repo
    /// merge does), so the exact-key matcher can attribute an edge to repos.
    fn graphql_details_in_repo(key: OperationKey, file: &str, repo: &str) -> ApiEndpointDetails {
        ApiEndpointDetails {
            repo_name: Some(repo.to_string()),
            ..graphql_details(key, file)
        }
    }

    #[test]
    fn test_graphql_matching_verified_missing_and_orphaned() {
        use crate::operation::GraphqlOperationKind;
        let cm = Lrc::new(SourceMap::default());
        let mut analyzer = Analyzer::new(Config::default(), cm);

        analyzer.endpoints.push(graphql_details(
            OperationKey::graphql(GraphqlOperationKind::Query, "user"),
            "schema.graphql:3",
        ));
        analyzer.endpoints.push(graphql_details(
            OperationKey::graphql(GraphqlOperationKind::Mutation, "createUser"),
            "schema.graphql:8",
        ));
        analyzer.calls.push(graphql_details(
            OperationKey::graphql(GraphqlOperationKind::Query, "user"),
            "client.ts:12",
        ));
        analyzer.calls.push(graphql_details(
            OperationKey::graphql(GraphqlOperationKind::Query, "orders"),
            "client.ts:20",
        ));

        let (call_issues, endpoint_issues, verified, _edges) =
            analyzer.analyze_exact_key_matches(crate::operation::Protocol::Graphql, "GraphQL");

        assert_eq!(verified, vec![("QUERY".to_string(), "user".to_string())]);
        assert_eq!(call_issues.len(), 1);
        assert!(
            call_issues[0].contains("Missing endpoint for QUERY orders"),
            "got {}",
            call_issues[0]
        );
        assert_eq!(endpoint_issues.len(), 1);
        assert_eq!(endpoint_issues[0].method, "MUTATION");
        assert_eq!(endpoint_issues[0].path, "createUser");
        // GraphQL orphans are not repo-tagged at this layer.
        assert!(endpoint_issues[0].service.is_none());
    }

    #[test]
    fn test_graphql_consumers_silent_without_indexed_producers() {
        use crate::operation::GraphqlOperationKind;
        let cm = Lrc::new(SourceMap::default());
        let mut analyzer = Analyzer::new(Config::default(), cm);

        // A consumer document but no GraphQL schema indexed anywhere: the
        // producing service may simply not be scanned — stay silent.
        analyzer.calls.push(graphql_details(
            OperationKey::graphql(GraphqlOperationKind::Query, "user"),
            "client.ts:12",
        ));

        let (call_issues, endpoint_issues, verified, edges) =
            analyzer.analyze_exact_key_matches(crate::operation::Protocol::Graphql, "GraphQL");
        assert!(call_issues.is_empty());
        assert!(endpoint_issues.is_empty());
        assert!(verified.is_empty());
        assert!(edges.is_empty());
    }

    #[test]
    fn test_socket_matching_is_direction_aware() {
        use crate::operation::SocketDirection;
        let cm = Lrc::new(SourceMap::default());
        let mut analyzer = Analyzer::new(Config::default(), cm);

        // Server side: listens for chat:message, emits chat:broadcast.
        analyzer.endpoints.push(graphql_details(
            OperationKey::socket("chat:message", SocketDirection::ClientToServer),
            "server.ts:10",
        ));
        analyzer.calls.push(graphql_details(
            OperationKey::socket("chat:broadcast", SocketDirection::ServerToClient),
            "server.ts:11",
        ));
        // Client side: emits chat:message, listens for chat:broadcast,
        // and emits one event nobody handles.
        analyzer.calls.push(graphql_details(
            OperationKey::socket("chat:message", SocketDirection::ClientToServer),
            "client.ts:5",
        ));
        analyzer.endpoints.push(graphql_details(
            OperationKey::socket("chat:broadcast", SocketDirection::ServerToClient),
            "client.ts:6",
        ));
        analyzer.calls.push(graphql_details(
            OperationKey::socket("typing", SocketDirection::ClientToServer),
            "client.ts:9",
        ));

        let (call_issues, endpoint_issues, verified, _edges) =
            analyzer.analyze_exact_key_matches(crate::operation::Protocol::Websocket, "Socket.IO");

        assert_eq!(
            verified,
            vec![
                ("CLIENT->SERVER".to_string(), "chat:message".to_string()),
                ("SERVER->CLIENT".to_string(), "chat:broadcast".to_string()),
            ]
        );
        assert_eq!(call_issues.len(), 1);
        assert!(
            call_issues[0].contains("Missing endpoint for CLIENT->SERVER typing"),
            "got {}",
            call_issues[0]
        );
        assert!(endpoint_issues.is_empty());
    }

    #[test]
    fn test_exact_key_matches_emit_cross_repo_edges() {
        use crate::operation::{GraphqlOperationKind, SocketDirection};
        let cm = Lrc::new(SourceMap::default());
        let mut analyzer = Analyzer::new(Config::default(), cm);

        // GraphQL: producer schema field in `gateway`, consumer document field
        // in `web-frontend`. Same operation key on both sides.
        analyzer.endpoints.push(graphql_details_in_repo(
            OperationKey::graphql(GraphqlOperationKind::Query, "order"),
            "schema.graphql:3",
            "gateway",
        ));
        analyzer.calls.push(graphql_details_in_repo(
            OperationKey::graphql(GraphqlOperationKind::Query, "order"),
            "web/lib/graphql.ts:5",
            "web-frontend",
        ));
        // Socket: the producer is the LISTENER (an endpoint) in `web-frontend`;
        // the consumer is the EMITTER (a call) in `payments-svc`. The event flows
        // payments-svc → web-frontend, but the contract producer is the listener.
        analyzer.endpoints.push(graphql_details_in_repo(
            OperationKey::socket("payment:settled", SocketDirection::ServerToClient),
            "web/lib/realtime.ts:8",
            "web-frontend",
        ));
        analyzer.calls.push(graphql_details_in_repo(
            OperationKey::socket("payment:settled", SocketDirection::ServerToClient),
            "payments/realtime/server.ts:9",
            "payments-svc",
        ));

        let (_, _, _, gql_edges) =
            analyzer.analyze_exact_key_matches(crate::operation::Protocol::Graphql, "GraphQL");
        assert_eq!(gql_edges.len(), 1, "one graphql edge expected");
        let e = &gql_edges[0];
        assert_eq!(e.producer_repo, "gateway");
        assert_eq!(e.consumer_repo, "web-frontend");
        assert_eq!(e.producer_key, "graphql|query|order");
        assert_eq!(e.consumer_key, "graphql|query|order");
        assert_eq!(e.match_score, 1.0);
        // Compat is filled in later by overlay_compat_verdicts, not here.
        assert_eq!(e.type_compatible, None);

        let (_, _, _, socket_edges) =
            analyzer.analyze_exact_key_matches(crate::operation::Protocol::Websocket, "Socket.IO");
        assert_eq!(socket_edges.len(), 1, "one socket edge expected");
        let s = &socket_edges[0];
        // Direction-aware: listener repo is the producer, emitter repo the consumer.
        assert_eq!(s.producer_repo, "web-frontend");
        assert_eq!(s.consumer_repo, "payments-svc");
        assert_eq!(s.producer_key, "socket|SERVER->CLIENT|payment:settled");
        assert_eq!(s.consumer_key, "socket|SERVER->CLIENT|payment:settled");
    }

    #[test]
    fn test_exact_key_matches_emit_edge_per_producer_repo() {
        use crate::operation::GraphqlOperationKind;
        let cm = Lrc::new(SourceMap::default());
        let mut analyzer = Analyzer::new(Config::default(), cm);

        // Two services expose the same GraphQL field; exact-key matching cannot
        // disambiguate by URL, so a consumer of `order` gets an edge to each.
        analyzer.endpoints.push(graphql_details_in_repo(
            OperationKey::graphql(GraphqlOperationKind::Query, "order"),
            "gateway/schema.graphql:3",
            "gateway",
        ));
        analyzer.endpoints.push(graphql_details_in_repo(
            OperationKey::graphql(GraphqlOperationKind::Query, "order"),
            "legacy/schema.graphql:3",
            "legacy-gateway",
        ));
        analyzer.calls.push(graphql_details_in_repo(
            OperationKey::graphql(GraphqlOperationKind::Query, "order"),
            "web/lib/graphql.ts:5",
            "web-frontend",
        ));

        let (_, _, _, edges) =
            analyzer.analyze_exact_key_matches(crate::operation::Protocol::Graphql, "GraphQL");
        assert_eq!(edges.len(), 2, "one edge per producer repo expected");
        let producer_repos: std::collections::BTreeSet<&str> =
            edges.iter().map(|e| e.producer_repo.as_str()).collect();
        assert_eq!(
            producer_repos,
            ["gateway", "legacy-gateway"].into_iter().collect()
        );
        assert!(edges.iter().all(|e| e.consumer_repo == "web-frontend"));
    }

    #[test]
    fn test_graphql_calls_do_not_hit_http_matcher() {
        use crate::operation::GraphqlOperationKind;
        let cm = Lrc::new(SourceMap::default());
        let mut analyzer = Analyzer::new(Config::default(), cm);

        analyzer.calls.push(graphql_details(
            OperationKey::graphql(GraphqlOperationKind::Query, "user"),
            "client.ts:12",
        ));

        let mount_graph = MountGraph::new();
        let (call_issues, endpoint_issues, env_var_calls, verified, _cross_repo_matches) =
            analyzer.analyze_matches_with_mount_graph(&mount_graph);
        assert!(call_issues.is_empty());
        assert!(endpoint_issues.is_empty());
        assert!(env_var_calls.is_empty());
        assert!(verified.is_empty());
    }

    #[test]
    fn test_analyze_matches_with_mount_graph_env_vars() {
        // Setup config with internal env vars
        let config = Config {
            internal_env_vars: ["API_URL".to_string()].into_iter().collect(),
            ..Config::default()
        };

        // Create analyzer with dummy source map (not used for this analysis)
        let cm = Lrc::new(SourceMap::default());
        let mut analyzer = Analyzer::new(config, cm);

        // Add calls that use env vars
        // 1. Valid internal call (should match if endpoint exists, or report missing)
        analyzer.calls.push(ApiEndpointDetails {
            owner: None,
            key: OperationKey::http("GET", "ENV_VAR:API_URL:/users"),
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: None,
            request_type: None,
            response_type: None,
            file_path: PathBuf::from("test.ts"),
            repo_name: None,
            service_name: None,
        });

        // 2. Unclassified env var (not in internal/external list)
        analyzer.calls.push(ApiEndpointDetails {
            owner: None,
            key: OperationKey::http("GET", "ENV_VAR:UNKNOWN_VAR:/posts"),
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: None,
            request_type: None,
            response_type: None,
            file_path: PathBuf::from("test.ts"),
            repo_name: None,
            service_name: None,
        });

        // 3. Process.env pattern (should be detected as env var)
        analyzer.calls.push(ApiEndpointDetails {
            owner: None,
            key: OperationKey::http("GET", "${process.env.OTHER_VAR}/comments"),
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: None,
            request_type: None,
            response_type: None,
            file_path: PathBuf::from("test.ts"),
            repo_name: None,
            service_name: None,
        });

        // 4. Raw code pattern with UPPERCASE var (common in legacy code)
        // e.g. LEGACY_API_URL + "/users"
        analyzer.calls.push(ApiEndpointDetails {
            owner: None,
            key: OperationKey::http("GET", "LEGACY_API_URL + \"/users\""),
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: None,
            request_type: None,
            response_type: None,
            file_path: PathBuf::from("test.ts"),
            repo_name: None,
            service_name: None,
        });

        let mount_graph = MountGraph::new(); // Empty graph

        // Run analysis
        let (call_issues, _, env_var_calls, _verified, _cross_repo_matches) =
            analyzer.analyze_matches_with_mount_graph(&mount_graph);

        // Check results
        // 1. Valid internal call should be in call_issues (missing endpoint) because graph is empty
        // Note: The analyzer normalizes the path for the error message
        assert!(
            call_issues
                .iter()
                .any(|i| i.contains("Missing endpoint") && i.contains("/users"))
        );

        // 2. Unclassified var should be in env_var_calls
        assert!(
            env_var_calls
                .iter()
                .any(|i| i.contains("Unclassified env var") && i.contains("UNKNOWN_VAR"))
        );

        // 3. Process.env var should be in env_var_calls
        assert!(
            env_var_calls
                .iter()
                .any(|i| i.contains("Unclassified env var") && i.contains("OTHER_VAR"))
        );

        // 4. Raw, unresolved `LEGACY_API_URL + "/users"` expressions are now
        // dropped by is_valid_route_shape: the file-analyzer contract requires
        // composed URLs to be normalized to `${VAR}/path`, so a raw JS
        // expression here is unreliable. This is the same tightening that stops
        // bare uppercase identifiers (`CarrickApiKeys`, `DynamoDB`) from being
        // mis-reported as env-var calls.
        assert!(!env_var_calls.iter().any(|i| i.contains("LEGACY_API_URL")));
    }

    // -----------------------------------------------------------------------
    // overlay_compat_verdicts — the S1 verdict-attachment step (#226). These
    // tests are deterministic: they craft a `type-check-results.json` in a temp
    // ts_check output dir and assert how its verdicts land on the edges. No
    // ts-node, no sidecar, no LLM — just the Rust overlay logic that #226's
    // offline harness exercises end-to-end.
    // -----------------------------------------------------------------------

    /// Minimal analyzer with `ts_check_dir` pointed at `dir`, and `results_json`
    /// written to `dir/output/type-check-results.json` (when `Some`).
    fn analyzer_with_results(dir: &std::path::Path, results_json: Option<&str>) -> Analyzer {
        let mut analyzer = Analyzer::new(Config::default(), Default::default());
        analyzer.set_ts_check_dir(dir.to_path_buf());
        if let Some(json) = results_json {
            let out = dir.join("output");
            std::fs::create_dir_all(&out).expect("create output dir");
            std::fs::write(out.join("type-check-results.json"), json).expect("write results");
        }
        analyzer
    }

    /// A `payments-svc` consumer edge against `producer_key`, with a default
    /// consumer call site. The overlay now keys per-consumer, so the verdict
    /// fixtures must carry a `consumerLocation` matching this location.
    const PAYMENTS_CONSUMER_LOC: &str = "payments-svc/src/client.ts:12:1";

    fn edge(producer_key: &str) -> CrossRepoMatch {
        edge_at(producer_key, "payments-svc", PAYMENTS_CONSUMER_LOC)
    }

    /// Build an edge with an explicit consumer repo + source location, so a test
    /// can model two consumers of one producer with distinct call sites.
    fn edge_at(producer_key: &str, consumer_repo: &str, consumer_location: &str) -> CrossRepoMatch {
        CrossRepoMatch {
            producer_repo: "orders-monorepo".to_string(),
            producer_key: producer_key.to_string(),
            consumer_repo: consumer_repo.to_string(),
            consumer_key: producer_key.to_string(),
            consumer_location: Some(consumer_location.to_string()),
            match_score: 1.0,
            type_compatible: None,
            mismatch_reason: None,
        }
    }

    /// With a results file present, every edge gets a verdict: the producer named
    /// in the mismatch list → `Some(false)` + reason; all others → `Some(true)`.
    /// This is the #226 happy path — the offline harness reaches HERE once
    /// ts_check actually runs and writes its results.
    #[test]
    fn overlay_compat_verdicts_attaches_from_results_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        // GET /orders/:id is incompatible; POST /payments is not in the mismatch
        // list, so it is compatible.
        let results = r#"{
            "mismatches": [
                { "endpoint": "GET /orders/:id (response)",
                  "consumerLocation": "payments-svc/src/client.ts:12",
                  "error": "id: number is not assignable to string" }
            ],
            "totalChecked": 2,
            "compatibleCount": 1
        }"#;
        let analyzer = analyzer_with_results(dir.path(), Some(results));

        let mut matches = vec![edge("http|GET|/orders/:id"), edge("http|POST|/payments")];
        analyzer.overlay_compat_verdicts(&mut matches);

        assert_eq!(
            matches[0].type_compatible,
            Some(false),
            "the producer in the mismatch list is incompatible"
        );
        assert_eq!(
            matches[0].mismatch_reason.as_deref(),
            Some("id: number is not assignable to string"),
        );
        assert_eq!(
            matches[1].type_compatible,
            Some(true),
            "a producer absent from the mismatch list is compatible (verdict reached)"
        );
        assert!(matches[1].mismatch_reason.is_none());
    }

    /// No results file → `check_type_compatibility` errs → every edge keeps
    /// `None` (the load-bearing absent verdict, NOT a fake `Some(true)`). This is
    /// exactly the state #226's live run was stuck in: ts_check never wrote
    /// results, so the verdicts were all absent and the §7 guard tripped.
    #[test]
    fn overlay_compat_verdicts_leaves_none_when_results_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        // output dir exists but no results file written.
        std::fs::create_dir_all(dir.path().join("output")).unwrap();
        let analyzer = analyzer_with_results(dir.path(), None);

        let mut matches = vec![edge("http|GET|/orders/:id")];
        analyzer.overlay_compat_verdicts(&mut matches);

        assert_eq!(
            matches[0].type_compatible, None,
            "no results file → compat not evaluated → verdict stays None (never fake true)"
        );
    }

    /// An edge ts_check matched but could NOT verify (a side resolved to
    /// `any`/`unknown`) lands in `unknownPairs`, not `mismatches`. Its verdict
    /// must stay `None` (unverifiable) rather than the optimistic `Some(true)` —
    /// asserting compatibility ts_check never established would mask a real
    /// shape mismatch hidden behind an `= unknown` placeholder (#235).
    #[test]
    fn overlay_compat_verdicts_leaves_unverifiable_edge_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        // GET /orders/:id is unverifiable (consumer resolved to `unknown`);
        // POST /payments was genuinely checked and is compatible.
        let results = r#"{
            "mismatches": [],
            "unknownPairs": [
                { "endpoint": "GET /orders/:id (response)",
                  "consumerLocation": "payments-svc/src/client.ts:12",
                  "reason": "consumer type resolves to unknown (type missing from bundled types?)" }
            ],
            "totalChecked": 1,
            "compatibleCount": 1
        }"#;
        let analyzer = analyzer_with_results(dir.path(), Some(results));

        let mut matches = vec![edge("http|GET|/orders/:id"), edge("http|POST|/payments")];
        analyzer.overlay_compat_verdicts(&mut matches);

        assert_eq!(
            matches[0].type_compatible, None,
            "an unverifiable edge stays None, never a fake Some(true)"
        );
        assert!(matches[0].mismatch_reason.is_none());
        assert_eq!(
            matches[1].type_compatible,
            Some(true),
            "a genuinely-checked edge absent from both lists is compatible"
        );
    }

    /// The `graphql|subscription|orderUpdated` false-positive at the verdict-join
    /// layer. Its consumer alias dangles to `unknown` in the bundle, so ts_check
    /// reports the pair in `unknownPairs` with the graphql endpoint label
    /// `"GRAPHQL subscription|orderUpdated (response)"`. That must join the edge
    /// whose `producer_key` is `"graphql|subscription|orderUpdated"` and pin the
    /// verdict to `None` — NOT the optimistic `Some(true)` the edge would default
    /// to if the unresolved consumer had been dropped from the manifest (so the
    /// pair never reached ts_check at all). This is the join the live eval's
    /// resolved graphql edges never exercise (they are compatible by default).
    #[test]
    fn apply_compat_verdicts_graphql_unverifiable_edge_none() {
        let result = serde_json::json!({
            "mismatches": [],
            "unknownPairs": [{
                "endpoint": "GRAPHQL subscription|orderUpdated (response)",
                "consumerLocation": "web-frontend/lib/graphql.ts:84",
                "reason": "consumer type resolves to unknown (type missing from bundled types?)"
            }],
            "totalChecked": 1,
            "compatibleCount": 0
        });
        let mut matches = vec![
            edge_at(
                "graphql|subscription|orderUpdated",
                "web-frontend",
                "web-frontend/lib/graphql.ts:84",
            ),
            edge_at(
                "graphql|query|order",
                "web-frontend",
                "web-frontend/lib/graphql.ts:76",
            ),
        ];
        apply_compat_verdicts(&result, &mut matches);

        assert_eq!(
            matches[0].type_compatible, None,
            "an unresolved graphql consumer makes the edge unverifiable (None), \
             never a fake Some(true) from being dropped + absent"
        );
        assert!(matches[0].mismatch_reason.is_none());
        assert_eq!(
            matches[1].type_compatible,
            Some(true),
            "the resolved graphql edge, absent from both lists, stays compatible"
        );
    }

    /// THE live compat=1/6 regression. ts_check builds its `endpoint` from the
    /// normalized manifest (`/orders/:param`), while the cross-repo edge's
    /// `producer_key` keeps the SOURCE param name (`/orders/:id`). The verdict
    /// join must collapse both to `:param`; otherwise the incompatible verdict
    /// misses the edge and it falls back to a fake `Some(true)`. With both sides
    /// using the same param name (as the older tests do), this asymmetry is
    /// invisible — which is exactly why it survived to the live eval.
    #[test]
    fn apply_compat_verdicts_joins_across_param_name_normalization() {
        let result = serde_json::json!({
            "mismatches": [{
                "endpoint": "GET /orders/:param (response)",
                "consumerLocation": "web-frontend/lib/api.ts:36",
                "error": "Order is not assignable to OrderView"
            }],
            "unknownPairs": [{
                "endpoint": "POST /payments (request)",
                "consumerLocation": "web-frontend/lib/api.ts:48"
            }]
        });
        let mut matches = vec![
            edge_at(
                "http|GET|/orders/:id",
                "web-frontend",
                "web-frontend/lib/api.ts:36",
            ),
            edge_at(
                "http|GET|/orders/:id",
                "payments-svc",
                "payments-svc/clients/orders.client.ts:13",
            ),
            edge_at(
                "http|POST|/payments",
                "web-frontend",
                "web-frontend/lib/api.ts:48",
            ),
            edge_at(
                "graphql|query|order",
                "web-frontend",
                "web-frontend/lib/graphql.ts:50",
            ),
        ];

        apply_compat_verdicts(&result, &mut matches);

        assert_eq!(
            matches[0].type_compatible,
            Some(false),
            "the web consumer's incompatible verdict must attach despite :id vs :param"
        );
        assert_eq!(
            matches[1].type_compatible,
            Some(true),
            "the payments consumer of the same producer keeps its own compatible verdict"
        );
        assert_eq!(
            matches[2].type_compatible, None,
            "POST /payments is unverifiable → None, never a fake compatible"
        );
        assert_eq!(
            matches[3].type_compatible,
            Some(true),
            "a graphql edge is now ts_check-verifiable and is absent from both \
             lists → genuinely-checked compatible (Some(true)), not None"
        );
    }

    #[test]
    fn normalize_compat_path_collapses_param_syntaxes() {
        assert_eq!(normalize_compat_path("/orders/:id"), "/orders/:param");
        assert_eq!(normalize_compat_path("/orders/{id}"), "/orders/:param");
        assert_eq!(normalize_compat_path("/orders/[id]"), "/orders/:param");
        assert_eq!(normalize_compat_path("/payments"), "/payments");
        assert_eq!(normalize_compat_path("/"), "/");
    }

    /// THE #260 regression. One producer endpoint (`GET /orders/:id`) with TWO
    /// consumers of differing compatibility: `payments-svc` (compatible) and
    /// `web-frontend` (incompatible — `Order.id:number` vs `OrderView.id:string`).
    /// ts_check emits one entry per pair, each carrying its own `consumerLocation`.
    /// Before the fix the overlay keyed verdicts on the producer `(METHOD, path)`
    /// alone, so whichever verdict landed first smeared onto BOTH edges — the
    /// `payments` `compatible` verdict masked the real `web-frontend` mismatch as
    /// `Some(true)` (the flagship false-negative). The verdicts must now be
    /// distinct and correctly attributed to each consumer's edge.
    #[test]
    fn overlay_compat_verdicts_keys_per_consumer_no_smear() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Only the web-frontend pair mismatches; the payments pair is genuinely
        // checked-and-compatible (absent from both lists). Both pairs share the
        // SAME producer endpoint — the exact collapse the old keying caused.
        let results = r#"{
            "mismatches": [
                { "endpoint": "GET /orders/:id (response)",
                  "consumerLocation": "web-frontend/src/orders.ts:42:3",
                  "error": "Order.id: number is not assignable to OrderView.id: string" }
            ],
            "unknownPairs": [],
            "totalChecked": 2,
            "compatibleCount": 1
        }"#;
        let analyzer = analyzer_with_results(dir.path(), Some(results));

        // Two edges into the SAME producer endpoint, distinguished only by their
        // consumer identity (repo + call-site location).
        let payments = edge_at(
            "http|GET|/orders/:id",
            "payments-svc",
            "payments-svc/src/orders-client.ts:18:5",
        );
        let web = edge_at(
            "http|GET|/orders/:id",
            "web-frontend",
            "web-frontend/src/orders.ts:42",
        );
        let mut matches = vec![payments, web];
        analyzer.overlay_compat_verdicts(&mut matches);

        // payments-svc: compatible (its pair is in neither list).
        assert_eq!(
            matches[0].consumer_repo, "payments-svc",
            "fixture ordering sanity"
        );
        assert_eq!(
            matches[0].type_compatible,
            Some(true),
            "the compatible consumer keeps its true verdict"
        );
        assert!(matches[0].mismatch_reason.is_none());

        // web-frontend: incompatible — and crucially NOT smeared with payments'
        // compatible verdict (the #260 false-negative).
        assert_eq!(matches[1].consumer_repo, "web-frontend");
        assert_eq!(
            matches[1].type_compatible,
            Some(false),
            "the incompatible consumer's edge must read Some(false), not the \
             smeared Some(true) — this is the #260 collapse"
        );
        assert_eq!(
            matches[1].mismatch_reason.as_deref(),
            Some("Order.id: number is not assignable to OrderView.id: string"),
        );
    }

    /// A GraphQL edge is now type-checked by ts_check (the graphql-compat
    /// machinery), so its `producer_key` joins a verdict just like socket. With
    /// the edge absent from both the mismatch and unknown lists, ts_check
    /// genuinely checked it and found it compatible, so the overlay reads
    /// `Some(true)` — NOT the old `None` (which meant "never checked") and NOT a
    /// fabricated true. The incompatible direction is pinned by
    /// `apply_compat_verdicts_joins_graphql_edge`.
    #[test]
    fn overlay_compat_verdicts_graphql_edge_joins_compatible() {
        let dir = tempfile::tempdir().expect("tempdir");
        let results = r#"{ "mismatches": [], "unknownPairs": [],
                           "totalChecked": 1, "compatibleCount": 1 }"#;
        let analyzer = analyzer_with_results(dir.path(), Some(results));

        let mut matches = vec![edge_at(
            "graphql|query|order",
            "web-frontend",
            "web-frontend/src/query.ts:7",
        )];
        analyzer.overlay_compat_verdicts(&mut matches);

        assert_eq!(
            matches[0].type_compatible,
            Some(true),
            "a GraphQL edge genuinely checked and absent from both lists is \
             compatible (Some(true)), now that graphql joins its verdict"
        );
        assert!(matches[0].mismatch_reason.is_none());
    }

    /// The socket cross-repo join (this PR). A `socket|DIRECTION|event` edge is
    /// now type-checked by ts_check, which emits its verdict under the endpoint
    /// label `"SOCKET <DIRECTION>|<event> (response)"`. `parse_producer_key`
    /// recovers `("SOCKET", "<DIRECTION>|<event>")` from the edge and
    /// `parse_compat_endpoint` recovers the SAME pair from the label, so the
    /// verdict lands on the socket edge — `Some(false)` + reason when ts_check
    /// reports a mismatch, `Some(true)` when it doesn't.
    #[test]
    fn apply_compat_verdicts_joins_socket_edge() {
        // The xrepo-corpus-1 `payment:settled` edge: producer (listener) is
        // web-frontend, consumer (emitter) is payments-svc. The compatible case
        // is absent from both lists, so it reads Some(true). The mismatch case
        // (a second event) lands on its edge with the reason.
        let result = serde_json::json!({
            "mismatches": [{
                "endpoint": "SOCKET CLIENT->SERVER|chat:bad (response)",
                "consumerLocation": "client/src/chat.ts:20",
                "error": "Sent type is not assignable to listener type"
            }],
            "unknownPairs": [],
            "totalChecked": 2,
            "compatibleCount": 1
        });

        let mut matches = vec![
            edge_at(
                "socket|SERVER->CLIENT|payment:settled",
                "payments-svc",
                "payments-svc/realtime/server.ts:27",
            ),
            edge_at(
                "socket|CLIENT->SERVER|chat:bad",
                "chat-svc",
                "client/src/chat.ts:20:5",
            ),
        ];
        apply_compat_verdicts(&result, &mut matches);

        assert_eq!(
            matches[0].type_compatible,
            Some(true),
            "the compatible socket edge (absent from both lists) reads Some(true)"
        );
        assert!(matches[0].mismatch_reason.is_none());

        assert_eq!(
            matches[1].type_compatible,
            Some(false),
            "the socket edge in the mismatch list reads Some(false)"
        );
        assert_eq!(
            matches[1].mismatch_reason.as_deref(),
            Some("Sent type is not assignable to listener type"),
        );
    }

    /// `parse_producer_key` recovers the `(pseudo-method, identity)` join pair
    /// from a graphql canonical key. The KIND stays lowercase on BOTH the key and
    /// the ts_check endpoint label (`GRAPHQL query|order (response)`), so the two
    /// sides agree with no case folding on the identity tail.
    #[test]
    fn parse_producer_key_recovers_graphql_pair() {
        assert_eq!(
            parse_producer_key("graphql|query|order"),
            Some(("GRAPHQL".to_string(), "query|order".to_string())),
        );
        assert_eq!(
            parse_producer_key("graphql|subscription|orderUpdated"),
            Some((
                "GRAPHQL".to_string(),
                "subscription|orderUpdated".to_string()
            )),
        );
        // A round-trip through the ts_check endpoint label must yield the SAME
        // pair, so a graphql verdict joins its edge.
        assert_eq!(
            parse_compat_endpoint("GRAPHQL query|order (response)"),
            parse_producer_key("graphql|query|order"),
        );
        // Malformed (missing field) → no join key, edge stays None.
        assert_eq!(parse_producer_key("graphql|query|"), None);
        assert_eq!(parse_producer_key("graphql|query"), None);
    }

    /// The graphql cross-repo join (the graphql-compat machinery). A
    /// `graphql|KIND|field` edge is type-checked by ts_check, which emits its
    /// verdict under the endpoint label `"GRAPHQL <KIND>|<field> (response)"`.
    /// `parse_producer_key` recovers `("GRAPHQL", "<KIND>|<field>")` from the edge
    /// and `parse_compat_endpoint` recovers the SAME pair from the label, so the
    /// verdict lands on the graphql edge — `Some(true)` when ts_check finds it
    /// compatible, `Some(false)` + reason when it reports a mismatch.
    #[test]
    fn apply_compat_verdicts_joins_graphql_edge() {
        // The xrepo-corpus-1 graphql edges: `query|order` is compatible (absent
        // from both lists → Some(true)); a second field is in the mismatch list
        // and lands on its edge with the reason.
        let result = serde_json::json!({
            "mismatches": [{
                "endpoint": "GRAPHQL subscription|orderUpdated (response)",
                "consumerLocation": "web-frontend/lib/graphql.ts:80",
                "error": "note?: optional producer field is not assignable to required consumer field"
            }],
            "unknownPairs": [],
            "totalChecked": 2,
            "compatibleCount": 1
        });

        let mut matches = vec![
            edge_at(
                "graphql|query|order",
                "web-frontend",
                "web-frontend/lib/graphql.ts:76",
            ),
            edge_at(
                "graphql|subscription|orderUpdated",
                "web-frontend",
                "web-frontend/lib/graphql.ts:80:5",
            ),
        ];
        apply_compat_verdicts(&result, &mut matches);

        assert_eq!(
            matches[0].type_compatible,
            Some(true),
            "the compatible graphql edge (absent from both lists) reads Some(true)"
        );
        assert!(matches[0].mismatch_reason.is_none());

        assert_eq!(
            matches[1].type_compatible,
            Some(false),
            "the graphql edge in the mismatch list reads Some(false)"
        );
        assert_eq!(
            matches[1].mismatch_reason.as_deref(),
            Some("note?: optional producer field is not assignable to required consumer field"),
        );
    }

    /// A results file carrying an `error` key (ts_check's catch-block output, e.g.
    /// the compile failure when its deps aren't installed) is treated as compat
    /// NOT evaluated → edges stay `None`, never scored as compatible.
    #[test]
    fn overlay_compat_verdicts_treats_error_result_as_unevaluated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let results = r#"{ "mismatches": [], "error": "Cannot find module 'ts-morph'" }"#;
        let analyzer = analyzer_with_results(dir.path(), Some(results));

        let mut matches = vec![edge("http|POST|/payments")];
        analyzer.overlay_compat_verdicts(&mut matches);

        assert_eq!(
            matches[0].type_compatible, None,
            "an error result is not a verdict — edges stay None"
        );
    }
}
