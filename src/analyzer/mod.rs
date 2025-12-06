pub mod builder;

use serde_json::Value;
use swc_common::{FileName, SourceMap, SourceMapper, Spanned, sync::Lrc};
use swc_ecma_ast::TsTypeAnn;

use crate::{
    app_context::AppContext,
    config::{Config, create_standard_tsconfig},
    extractor::CoreExtractor,
    mount_graph::MountGraph,
    packages::Packages,
    url_normalizer::UrlNormalizer,
    utils::{get_repository_name, join_prefix_and_path},
    visitor::{Call, FunctionDefinition, FunctionNodeType, Json, Mount, OwnerType, TypeReference},
};
use std::collections::HashSet;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

// Type aliases to reduce complexity
type RouteFieldMap = HashMap<(String, String), Json>;

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

pub struct ApiIssues {
    pub call_issues: Vec<String>,
    pub endpoint_issues: Vec<String>,
    pub env_var_calls: Vec<String>,
    pub mismatches: Vec<String>,
    pub type_mismatches: Vec<String>,
    pub dependency_conflicts: Vec<DependencyConflict>,
}

impl ApiIssues {
    pub fn is_empty(&self) -> bool {
        self.call_issues.is_empty()
            && self.endpoint_issues.is_empty()
            && self.type_mismatches.is_empty()
            && self.dependency_conflicts.is_empty()
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ApiEndpointDetails {
    // owner is Option as we store both call ands endpoints in this data structure.
    // It might make sense to split this out into its own type
    pub owner: Option<OwnerType>,
    pub route: String,
    pub method: String,
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
        }
    }

    /// Set the mount graph for framework-agnostic analysis
    pub fn set_mount_graph(&mut self, mount_graph: MountGraph) {
        self.mount_graph = Some(mount_graph);
    }

    pub fn fetch_calls(&self) -> &Vec<Call> {
        &self.fetch_calls
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
        use crate::gemini_service::extract_calls_from_async_expressions;

        let mut all_async_contexts = Vec::new();

        // Extract async calls from each function definition using extractor methods
        for def in self.function_definitions.values() {
            let async_contexts = self.extract_async_calls_from_function(def);
            all_async_contexts.extend(async_contexts);
        }

        // Skip Gemini call if no async expressions found (safety check)
        if all_async_contexts.is_empty() {
            println!("No async expressions found, skipping Gemini analysis");
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
                eprintln!("Failed to extract calls from async expressions: {}", e);
                vec![]
            }
        };

        println!("Gemini extracted {} HTTP calls", gemini_calls.len());

        // Process calls as before
        let processed_calls = self.process_fetch_calls(gemini_calls);
        self.fetch_calls.extend(processed_calls.clone());

        // Create ApiEndpointDetails from processed calls
        for call in processed_calls {
            let params = self.extract_params_from_route(&call.route);
            self.calls.push(ApiEndpointDetails {
                owner: None,
                route: call.route.clone(),
                method: call.method.clone(),
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
        route
            .split('/')
            .filter(|segment| !segment.is_empty()) // Remove empty segments
            .map(|segment| {
                if let Some(param_name) = segment.strip_prefix(':') {
                    // Convert :id -> ById, :userId -> ByUserId, :eventId -> ByEventId
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
            eprintln!(
                "Warning: Span `lo` ({:?}) is before its supposed file's start_pos ({:?}) for file {:?}. This indicates a SourceMap or span issue.",
                type_ref_span.lo, file_start_bytepos, loc.file.name
            );
            return None; // Or handle as an error appropriately
        }
        let file_relative_byte_offset_u32 = (type_ref_span.lo - file_start_bytepos).0;

        let actual_span_file_path = match &*loc.file.name {
            FileName::Real(pathbuf) => pathbuf.clone(), // Clone to own PathBuf
            other => {
                eprintln!(
                    "Span found in a non-real file: {:?}. Cannot process.",
                    other
                );
                return None;
            }
        };

        let file_content = match std::fs::read_to_string(&actual_span_file_path) {
            Ok(content) => content,
            Err(e) => {
                eprintln!(
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
            let key = (
                endpoint.route.clone(),
                endpoint.method.clone(),
                endpoint.handler_name.clone(),
            );
            // returns true or false if the value in the set already exists
            seen.insert(key)
        });

        for endpoint in &self.endpoints {
            if let Some(handler_name) = &endpoint.handler_name {
                if let Some(func_def) = self.function_definitions.get(handler_name) {
                    if func_def.arguments.len() >= 2 {
                        // Process Request Type (argument 0)
                        if let Some(req_type_ann_swc) = &func_def.arguments[0].type_ann {
                            let alias = Self::generate_common_type_alias_name(
                                &endpoint.route,
                                &endpoint.method,
                                true,  // is_request_type
                                false, // is_consumer = false (endpoints are producers)
                            );
                            if let Some(type_ref) = Self::create_type_reference_from_swc(
                                req_type_ann_swc,
                                &cm,
                                &func_def.file_path,
                                alias,
                            ) {
                                request_types_map.insert(
                                    (endpoint.route.clone(), endpoint.method.clone()),
                                    type_ref,
                                );
                            }
                        }

                        // Process Response Type (argument 1)
                        if let Some(res_type_ann_swc) = &func_def.arguments[1].type_ann {
                            let alias = Self::generate_common_type_alias_name(
                                &endpoint.route,
                                &endpoint.method,
                                false, // is_request_type = false
                                false, // is_consumer = false (endpoints are producers)
                            );
                            if let Some(type_ref) = Self::create_type_reference_from_swc(
                                res_type_ann_swc,
                                &cm,
                                &func_def.file_path,
                                alias,
                            ) {
                                response_types_map.insert(
                                    (endpoint.route.clone(), endpoint.method.clone()),
                                    type_ref,
                                );
                            }
                        }
                    }
                }
            }
        }

        // Update all endpoints with the resolved types
        for endpoint in &mut self.endpoints {
            let key = (endpoint.route.clone(), endpoint.method.clone());
            if let Some(req_type) = request_types_map.get(&key) {
                endpoint.request_type = Some(req_type.clone());
            }
            if let Some(resp_type) = response_types_map.get(&key) {
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
                response_fields.insert((route.clone(), method.clone()), resp_json);
                if let Some(req) = req_json {
                    request_fields.insert((route.clone(), method.clone()), req);
                }
            }
        }

        (response_fields, request_fields)
    }

    // We know endpoints will exist for each imported handler
    pub fn update_endpoints_with_resolved_fields(
        &mut self,
        response_fields: HashMap<(String, String), Json>,
        request_fields: HashMap<(String, String), Json>,
    ) -> &mut Self {
        for endpoint in &mut self.endpoints {
            let key = (endpoint.route.clone(), endpoint.method.clone());
            if let Some(response) = response_fields.get(&key) {
                endpoint.response_body = Some(response.clone());
            }
            if let Some(request) = request_fields.get(&key) {
                endpoint.request_body = Some(request.clone());
            }
        }

        self
    }

    /// Framework-agnostic analysis using mount graph
    /// Finds orphaned endpoints and missing API calls without pattern matching
    fn analyze_matches_with_mount_graph(
        &self,
        mount_graph: &MountGraph,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let mut call_issues = Vec::new();
        let mut endpoint_issues = Vec::new();
        let mut env_var_calls = Vec::new();

        // Track which endpoints have been matched
        let mut matched_endpoints: HashSet<String> = HashSet::new();

        // Deduplicate calls
        let mut unique_calls = Vec::new();
        let mut seen_calls = HashSet::new();
        for call in &self.calls {
            let key = format!(
                "{}:{}:{}",
                call.method,
                call.route,
                call.file_path.display()
            );
            if seen_calls.insert(key) {
                unique_calls.push(call);
            }
        }

        // For each call, try to find matching endpoint using mount graph
        for call in &unique_calls {
            // Check for environment variable URLs (framework-agnostic)
            if call.route.contains("ENV_VAR:")
                || call.route.contains("process.env")
                || call.route.contains("${")
            {
                // Check if it's a configured external or internal call
                if self.config.is_external_call(&call.route) {
                    continue; // Skip external calls
                } else if !self.config.is_internal_call(&call.route) {
                    // Unknown env var URL
                    env_var_calls.push(format!(
                        "API call with environment variable URL: {} {} in {}",
                        call.method,
                        call.route,
                        call.file_path.display()
                    ));
                    continue;
                }
            }

            // Use mount graph to find matching endpoints with URL normalization
            // This handles full URLs, env var patterns, template literals, etc.
            let normalizer = UrlNormalizer::new(&self.config);

            match mount_graph.find_matching_endpoints_with_normalizer(
                &call.route,
                &call.method,
                &normalizer,
            ) {
                None => {
                    // URL was identified as external - skip it
                    continue;
                }
                Some(matching_endpoints) => {
                    if matching_endpoints.is_empty() {
                        // Extract normalized path for better error message
                        let normalized_path = normalizer.extract_path(&call.route);
                        call_issues.push(format!(
                            "Missing endpoint for {} {} (normalized: {}) (called from {})",
                            call.method,
                            call.route,
                            normalized_path,
                            call.file_path.display()
                        ));
                    } else {
                        // Mark endpoints as matched
                        for endpoint in matching_endpoints {
                            let key = format!("{}:{}", endpoint.method, endpoint.full_path);
                            matched_endpoints.insert(key);
                        }
                    }
                }
            }
        }

        // Find orphaned endpoints (not matched by any call)
        for endpoint in mount_graph.get_resolved_endpoints() {
            let key = format!("{}:{}", endpoint.method, endpoint.full_path);
            if !matched_endpoints.contains(&key) {
                endpoint_issues.push(format!(
                    "Orphaned endpoint: {} {} in {}",
                    endpoint.method, endpoint.full_path, endpoint.file_location
                ));
            }
        }

        (call_issues, endpoint_issues, env_var_calls)
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

        let mut path = endpoint.route.clone();
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
            let full_paths = Self::compute_full_paths_for_endpoint(endpoint, mounts, apps);
            for path in full_paths {
                let mut ep = endpoint.clone();
                ep.route = path;
                new_endpoints.push(ep);
            }
        }
        new_endpoints
    }

    fn normalize_route_params(&self, route: &str) -> String {
        // Use a regex to replace all parameter placeholders with a consistent name
        let param_regex = regex::Regex::new(r":([\w]+)").unwrap();
        param_regex.replace_all(route, "{param}").to_string()
    }

    pub fn build_endpoint_router(&mut self) {
        let mut router = matchit::Router::new();

        // Use a HashMap to collect all endpoints by path before inserting into router
        let mut path_to_endpoints: HashMap<String, Vec<(String, String)>> = HashMap::new();

        for endpoint in &self.endpoints {
            let normalized_route = self.normalize_route_params(&endpoint.route);

            path_to_endpoints
                .entry(normalized_route)
                .or_default()
                .push((endpoint.route.clone(), endpoint.method.clone()));
        }

        println!("Unique endpoint paths: {}", path_to_endpoints.len());

        // Now insert each unique path once, with a collection of route-method pairs
        for (path, route_methods) in path_to_endpoints {
            if let Err(e) = router.insert(&path, route_methods) {
                println!("Warning: Could not add route to router: {}", e);
            }
        }

        self.endpoint_router = Some(router);
    }

    pub fn extract_types_for_repo(
        &self,
        repo_path: &str,
        type_infos: Vec<Value>,
        packages: &Packages,
    ) {
        use std::process::Command;

        // Skip if no types to extract
        if type_infos.is_empty() {
            return;
        }

        // Prepare JSON input with type information
        let json_input = serde_json::to_string(&type_infos).unwrap();
        let repo_suffix = get_repository_name(repo_path);
        let output_path = format!("ts_check/output/{}_types.ts", repo_suffix);

        // Ensure the `ts_check/output` directory exists
        std::fs::create_dir_all("ts_check/output").expect("Failed to create output directory");

        let dependencies = packages.get_dependencies();
        // Serialize dependencies as JSON
        let dependencies_json = serde_json::to_string(dependencies).unwrap();

        // Determine tsconfig path based on repo
        use std::path::Path;
        let tsconfig_path = Path::new(repo_path).join("tsconfig.json");

        // Use repo's tsconfig if it exists, otherwise create a default one in output directory
        let ts_config = if tsconfig_path.exists() {
            tsconfig_path
        } else {
            println!("No tsconfig.json found in repo, creating default one in ts_check/output");

            // Ensure the output directory exists
            let output_dir = Path::new("ts_check/output");
            if !output_dir.exists() {
                std::fs::create_dir_all(output_dir).expect("Failed to create output directory");
            }

            let default_tsconfig_path = Path::new("ts_check/output/tsconfig.json");

            // Create a basic tsconfig.json content
            let tsconfig_content = create_standard_tsconfig();

            std::fs::write(
                default_tsconfig_path,
                serde_json::to_string_pretty(&tsconfig_content).unwrap(),
            )
            .expect("Failed to write default tsconfig.json");

            default_tsconfig_path.to_path_buf()
        };

        println!("Extracting {} types from {}", type_infos.len(), repo_path);

        // Run the extract-type-definitions script with all types at once
        let script_path = match std::fs::canonicalize("ts_check/extract-type-definitions.ts") {
            Ok(path) => path,
            Err(e) => panic!("Script not found: {}", e),
        };

        let ts_config = match std::fs::canonicalize(&ts_config) {
            Ok(path) => path,
            Err(e) => panic!("tsconfig.json not found: {}", e),
        };

        let output = Command::new("npx")
            .arg("ts-node")
            .arg(script_path)
            .arg(&json_input)
            .arg(&output_path)
            .arg(ts_config)
            .arg(&dependencies_json)
            .output()
            .expect("Failed to run type extraction");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        println!("Type extraction output: {}", stdout);
        if !stderr.is_empty() {
            eprintln!("Type extraction errors: {}", stderr);
        }
    }

    /// Collect type information from Gemini-extracted calls for TypeScript extraction
    pub fn collect_type_infos_from_calls(&self, calls: &[Call]) -> Vec<serde_json::Value> {
        println!("collect_type_infos_from_calls is called");
        let mut type_infos = Vec::new();

        for call in calls {
            // Collect request type info
            if let Some(request_type) = &call.request_type {
                let type_info = serde_json::json!({
                    "filePath": request_type.file_path.to_string_lossy().to_string(),
                    "startPosition": request_type.start_position,
                    "compositeTypeString": request_type.composite_type_string,
                    "alias": request_type.alias
                });
                type_infos.push(type_info);
            }

            // Collect response type info
            if let Some(response_type) = &call.response_type {
                let type_info = serde_json::json!({
                    "filePath": response_type.file_path.to_string_lossy().to_string(),
                    "startPosition": response_type.start_position,
                    "compositeTypeString": response_type.composite_type_string,
                    "alias": response_type.alias
                });
                type_infos.push(type_info);
            }
        }

        type_infos
    }

    pub fn check_type_compatibility(&self) -> Result<serde_json::Value, String> {
        use std::fs;
        use std::path::Path;

        // Ensure the output directory exists
        if !Path::new("ts_check/output").exists() {
            return Err("Output directory ts_check/output does not exist".to_string());
        }

        // Check for type-check-results.json file created by the integrated type checker
        let results_file = Path::new("ts_check/output/type-check-results.json");

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

        let (call_issues, endpoint_issues, env_var_calls) =
            self.analyze_matches_with_mount_graph(mount_graph);
        // Note: JSON body comparison removed - type checking is done via TypeScript (ts_check/)
        let mismatches = Vec::new();
        let type_mismatches = self.get_type_mismatches();
        let dependency_conflicts = self.analyze_dependencies();

        ApiAnalysisResult {
            endpoints: self.endpoints.clone(),
            calls: self.calls.clone(),
            issues: ApiIssues {
                call_issues,
                endpoint_issues,
                env_var_calls,
                mismatches,
                type_mismatches,
                dependency_conflicts,
            },
        }
    }

    pub fn run_final_type_checking(&self) -> Result<(), String> {
        use std::fs;
        use std::path::Path;
        use std::process::Command;

        // Check if we have the type checking script
        let script_path = "ts_check/run-type-checking.ts";
        if !Path::new(script_path).exists() {
            return Err("Type checking script not found".to_string());
        }

        // Create minimal tsconfig.json in output directory
        let output_dir = Path::new("ts_check/output");
        fs::create_dir_all(output_dir)
            .map_err(|e| format!("Failed to create output directory: {}", e))?;

        // Check if there are any type files to check
        let type_files: Vec<_> = fs::read_dir(output_dir)
            .map_err(|e| format!("Failed to read output directory: {}", e))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.path().extension().is_some_and(|ext| ext == "ts")
                    && entry
                        .path()
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().ends_with("_types.ts"))
            })
            .collect();

        if type_files.is_empty() {
            println!("⚠️  No type files found in ts_check/output/ - skipping type checking");
            println!("   This may happen if:");
            println!("   - Source code lacks explicit TypeScript type annotations");
            println!("   - Type extraction agents couldn't identify response/request types");
            println!("   - This is the first run and no cross-repo data exists yet");
            println!("   Type checking will work when type annotations are present in the source.");
            return Ok(());
        }

        println!(
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

        // Run the type checking script with the minimal tsconfig
        let output = Command::new("npx")
            .arg("ts-node")
            .arg(script_path)
            .arg(tsconfig_path)
            .output()
            .map_err(|e| format!("Failed to run type checking: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Print the output from the type checking script
        if !stdout.trim().is_empty() {
            print!("{}", stdout);
        }
        if !stderr.trim().is_empty() && !output.status.success() {
            print!("{}", stderr);
        }

        if !output.status.success() {
            return Err(format!(
                "Type checking script failed with exit code: {:?}",
                output.status.code()
            ));
        }

        Ok(())
    }

    fn get_type_mismatches(&self) -> Vec<String> {
        match self.check_type_compatibility() {
            Ok(result) => {
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
                                let clean_producer = self.clean_type_string(producer);
                                let clean_consumer = self.clean_type_string(consumer);
                                let clean_error = self.clean_error_message(error);

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

    fn clean_type_string(&self, type_str: &str) -> String {
        use regex::Regex;

        // Remove absolute paths from import statements, keeping only the relative part
        let import_regex = Regex::new(r#"import\("([^"]+)"\)\.(\w+)"#).unwrap();
        let mut cleaned = import_regex
            .replace_all(type_str, |caps: &regex::Captures| {
                let path = &caps[1];
                let type_name = &caps[2];
                // Extract just the filename without path for readability
                if let Some(filename) = path.split('/').last() {
                    format!("{}.{}", filename, type_name)
                } else {
                    format!("{}.{}", path, type_name)
                }
            })
            .to_string();

        // Simplify Array<T> to T[]
        let array_regex = Regex::new(r"Array<([^>]+)>").unwrap();
        cleaned = array_regex.replace_all(&cleaned, "$1[]").to_string();

        cleaned
    }

    fn clean_error_message(&self, error: &str) -> String {
        error
            .replace("Type '", "")
            .replace(
                "' is missing the following properties from type '",
                " missing properties from ",
            )
            .replace("': ", ": ")
            .replace("' is not assignable to type '", " not assignable to ")
            .replace("'.", "")
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

    /// Extract repository prefix from file path by matching against repository paths
    pub fn extract_repo_prefix_from_file_path(
        &self,
        file_path: &Path,
        repo_paths: &[String],
    ) -> String {
        let file_path_str = file_path.to_string_lossy();
        repo_paths
            .iter()
            .find(|repo_path| file_path_str.starts_with(*repo_path))
            .map(|repo_path| get_repository_name(repo_path))
            .unwrap_or("default".to_string())
    }

    /// Add a TypeReference to the repository type map with incremental naming for multiple calls
    fn add_type_to_repo_map(
        &self,
        type_ref: &TypeReference,
        repo_prefix: String,
        repo_type_map: &mut HashMap<String, Vec<Value>>,
    ) {
        let file_path = type_ref.file_path.to_string_lossy().to_string();

        let canonical_path =
            std::fs::canonicalize(file_path).expect("Cannot extract full file path");
        if let Some(path) = canonical_path.to_str() {
            let base_alias = &type_ref.alias;

            // For consumer types (calls), we need to ensure unique names
            let final_alias = if base_alias.ends_with("Response") || base_alias.ends_with("Request")
            {
                let repo_entries = repo_type_map.entry(repo_prefix.clone()).or_default();

                // Find all existing aliases that match this base pattern
                let existing_aliases: std::collections::HashSet<String> = repo_entries
                    .iter()
                    .filter_map(|entry| entry.get("alias").and_then(|v| v.as_str()))
                    .map(|s| s.to_string())
                    .collect();

                // If the base alias already exists, find the next available number
                if existing_aliases.contains(base_alias) {
                    let mut counter = 2;
                    loop {
                        let candidate = format!("{}{}", base_alias, counter);
                        if !existing_aliases.contains(&candidate) {
                            break candidate;
                        }
                        counter += 1;
                    }
                } else {
                    base_alias.clone()
                }
            } else {
                base_alias.clone()
            };

            let entry = serde_json::json!({
                "filePath": path.to_string(),
                "startPosition": type_ref.start_position,
                "compositeTypeString": type_ref.composite_type_string,
                "alias": final_alias
            });

            repo_type_map.entry(repo_prefix).or_default().push(entry);
        }
    }

    /// Process both request and response types for an ApiEndpointDetails
    pub fn process_api_detail_types(
        &self,
        api_detail: &ApiEndpointDetails,
        repo_prefix: String,
        repo_type_map: &mut HashMap<String, Vec<Value>>,
    ) {
        if let Some(req_type) = &api_detail.request_type {
            self.add_type_to_repo_map(req_type, repo_prefix.clone(), repo_type_map);
        }

        if let Some(resp_type) = &api_detail.response_type {
            self.add_type_to_repo_map(resp_type, repo_prefix, repo_type_map);
        }
    }
}
