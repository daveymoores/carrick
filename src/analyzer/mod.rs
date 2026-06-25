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

/// Parse a ts_check compat `endpoint` string back into `(METHOD, path)`. The
/// string is built as `"<METHOD> <path> (<request|response>)"` by ts_check's
/// type-checker, so split off the leading method and the trailing
/// `" (type_kind)"` suffix. Returns `None` for an unrecognized shape.
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

/// Recover `(METHOD, path)` from a canonical HTTP producer key
/// (`"http|METHOD|path"`). Returns `None` for non-HTTP keys.
fn parse_producer_key(key: &str) -> Option<(String, String)> {
    let mut parts = key.splitn(3, '|');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("http"), Some(method), Some(path)) if !method.is_empty() && !path.is_empty() => {
            Some((method.to_uppercase(), path.to_string()))
        }
        _ => None,
    }
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

                if has_conflicts {
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
        // and dedup the captured edges on their identity tuple.
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
        cross_repo_matches.dedup_by(|a, b| {
            a.producer_repo == b.producer_repo
                && a.producer_key == b.producer_key
                && a.consumer_repo == b.consumer_repo
                && a.consumer_key == b.consumer_key
        });

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
    ) -> (Vec<String>, Vec<OrphanedEndpoint>, Vec<(String, String)>) {
        let producer_keys: HashSet<&OperationKey> = self
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.key.protocol() == protocol)
            .map(|endpoint| &endpoint.key)
            .collect();
        if producer_keys.is_empty() {
            return (Vec::new(), Vec::new(), Vec::new());
        }

        let mut call_issues = Vec::new();
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

        (call_issues, endpoint_issues, verified)
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
                serde_json::json!({
                    "endpoint": mismatch.get("endpoint").unwrap_or(&serde_json::Value::Null),
                    "producerType": mismatch.get("producerType").unwrap_or(&serde_json::Value::Null),
                    "consumerType": mismatch.get("consumerType").unwrap_or(&serde_json::Value::Null),
                    "error": mismatch.get("error").unwrap_or(&serde_json::Value::Null)
                })
            })
            .collect::<Vec<_>>();

        Ok(serde_json::json!({
            "mismatches": mismatches,
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
            let (protocol_call_issues, protocol_endpoint_issues, protocol_verified) =
                self.analyze_exact_key_matches(protocol, label);
            call_issues.extend(protocol_call_issues);
            endpoint_issues.extend(protocol_endpoint_issues);
            verified_endpoints.extend(protocol_verified);
        }
        verified_endpoints.sort();
        verified_endpoints.dedup();
        // Note: JSON body comparison removed - type checking is done via TypeScript (ts_check/)
        let mismatches = Vec::new();
        let type_mismatches = self.get_type_mismatches();
        let dependency_conflicts = self.analyze_dependencies();

        // Overlay the per-producer-endpoint type-compat verdict onto the captured
        // edges. The verdict is keyed by the producer's (METHOD, full_path) — the
        // ts_check output collapses all consumers of a producer into one verdict,
        // so every edge into a given producer shares that producer's verdict.
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
    /// edge, keyed by the producer's `(METHOD, full_path)`.
    ///
    /// If `check_type_compatibility` returns `Err`, type checking did not run
    /// (or failed) for this scan: every edge keeps `type_compatible: None`
    /// (load-bearing — see [`CrossRepoMatch`]). On `Ok`, an edge whose producer
    /// appears in the mismatch set gets `Some(false)` + the reason; otherwise
    /// `Some(true)` (compat evaluated, no mismatch for that producer).
    fn overlay_compat_verdicts(&self, matches: &mut [CrossRepoMatch]) {
        let result = match self.check_type_compatibility() {
            Ok(result) => result,
            // Compat was not evaluated for this run — leave every edge `None`.
            Err(_) => return,
        };

        // Map producer `(METHOD, path)` → mismatch reason, parsed from each
        // mismatch's `endpoint` string (`"<METHOD> <path> (<request|response>)"`,
        // built by ts_check's type-checker). Multiple type_kinds per producer
        // collapse to the first reason seen — the edge only records that the
        // producer is incompatible.
        let mut incompatible: HashMap<(String, String), String> = HashMap::new();
        if let Some(mismatch_list) = result.get("mismatches").and_then(|m| m.as_array()) {
            for mismatch in mismatch_list {
                let Some(endpoint) = mismatch.get("endpoint").and_then(|e| e.as_str()) else {
                    continue;
                };
                let Some((method, path)) = parse_compat_endpoint(endpoint) else {
                    continue;
                };
                let reason = mismatch
                    .get("error")
                    .and_then(|e| e.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("producer and consumer types are incompatible")
                    .to_string();
                incompatible.entry((method, path)).or_insert(reason);
            }
        }

        for edge in matches.iter_mut() {
            // producer_key is `http|METHOD|path`; recover (METHOD, path).
            let Some((method, path)) = parse_producer_key(&edge.producer_key) else {
                edge.type_compatible = Some(true);
                continue;
            };
            match incompatible.get(&(method, path)) {
                Some(reason) => {
                    edge.type_compatible = Some(false);
                    edge.mismatch_reason = Some(reason.clone());
                }
                None => {
                    edge.type_compatible = Some(true);
                }
            }
        }
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

        // Run the type checking script with the minimal tsconfig
        let output = Command::new("npx")
            .arg("ts-node")
            .arg(&script_path)
            .arg(&tsconfig_path)
            .arg("--producer")
            .arg(&producer_manifest)
            .arg("--consumer")
            .arg(&consumer_manifest)
            .arg("--types-dir")
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
            return Err(format!(
                "Type checking script failed with exit code: {:?}",
                output.status.code()
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

        let (call_issues, endpoint_issues, verified) =
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

        let (call_issues, endpoint_issues, verified) =
            analyzer.analyze_exact_key_matches(crate::operation::Protocol::Graphql, "GraphQL");
        assert!(call_issues.is_empty());
        assert!(endpoint_issues.is_empty());
        assert!(verified.is_empty());
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

        let (call_issues, endpoint_issues, verified) =
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
}
