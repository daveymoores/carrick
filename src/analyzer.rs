use serde_json::Value;
use swc_common::{FileName, SourceMap, SourceMapper, Spanned, sync::Lrc};
use swc_ecma_ast::TsTypeAnn;

use crate::{
    app_context::AppContext,
    config::Config,
    extractor::CoreExtractor,
    packages::Packages,
    utils::join_prefix_and_path,
    visitor::{
        DependencyVisitor, FunctionDefinition, FunctionNodeType, Json, Mount, OwnerType,
        TypeReference,
    },
};
use core::fmt;
use std::collections::HashSet;
use std::{collections::HashMap, path::PathBuf};

pub struct ApiIssues {
    pub call_issues: Vec<String>,
    pub endpoint_issues: Vec<String>,
    pub env_var_calls: Vec<String>,
    pub mismatches: Vec<String>,
}

impl ApiIssues {
    pub fn is_empty(&self) -> bool {
        self.call_issues.is_empty() && self.endpoint_issues.is_empty()
    }

    pub fn len(&self) -> usize {
        self.call_issues.len() + self.endpoint_issues.len()
    }
}

#[derive(Clone, Debug)]
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

impl fmt::Display for FieldMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldMismatch::MissingField(field) => write!(f, "Missing field: {}", field),
            FieldMismatch::ExtraField(field) => write!(f, "Extra field: {}", field),
            FieldMismatch::TypeMismatch(path, call_type, endpoint_type) => write!(
                f,
                "Type mismatch at {}: call has type {}, endpoint expects type {}",
                path, call_type, endpoint_type
            ),
        }
    }
}

pub struct ApiAnalysisResult {
    pub endpoints: Vec<ApiEndpointDetails>,
    pub calls: Vec<ApiEndpointDetails>,
    pub issues: ApiIssues,
}

pub struct Analyzer {
    // <Route, http_method, handler_name, source>
    imported_handlers: Vec<(String, String, String, String)>,
    function_definitions: HashMap<String, FunctionDefinition>,
    endpoints: Vec<ApiEndpointDetails>,
    calls: Vec<ApiEndpointDetails>,
    mounts: Vec<Mount>,
    apps: HashMap<String, AppContext>,
    config: Config,
    endpoint_router: Option<matchit::Router<Vec<(String, String)>>>,
    source_map: Lrc<SourceMap>,
}

#[derive(Debug)]
pub enum FieldMismatch {
    MissingField(String),
    ExtraField(String),
    TypeMismatch(String, String, String), // (path, call_type, endpoint_type)
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
            mounts: Vec::new(),
            apps: HashMap::new(),
            config,
            endpoint_router: None,
            source_map,
        }
    }

    pub fn add_visitor_data(&mut self, visitor: DependencyVisitor) {
        self.mounts.extend(visitor.mounts);
        self.apps.extend(visitor.express_apps);

        for endpoint in visitor.endpoints {
            let params = self.extract_params_from_route(&endpoint.route);
            self.endpoints.push(ApiEndpointDetails {
                owner: Some(endpoint.owner.clone()),
                route: endpoint.route.to_string(),
                method: endpoint.method.to_string(),
                params,
                response_body: Some(endpoint.response),
                request_body: endpoint.request,
                handler_name: Some(endpoint.handler_name),
                request_type: endpoint.request_type,
                response_type: endpoint.response_type,
                file_path: endpoint.handler_file,
            });
        }

        // expected_fields being returned data from all CRUD calls
        for call in visitor.calls {
            let params = self.extract_params_from_route(&call.route);
            self.calls.push(ApiEndpointDetails {
                owner: None,
                route: call.route.to_string(),
                method: call.method.to_string(),
                params,
                response_body: Some(call.response),
                request_body: call.request,
                handler_name: None, // Calls don't typically have handlers
                request_type: call.request_type,
                response_type: call.response_type,
                file_path: call.call_file,
            })
        }

        self.imported_handlers
            .extend(visitor.imported_handlers.clone());

        for (name, def) in visitor.function_definitions {
            self.function_definitions.insert(name, def);
        }
    }

    pub fn analyze_functions_for_fetch_calls(&mut self) {
        let mut new_calls = Vec::new();

        // Clone the function_definitions to avoid borrowing issues
        let function_defs = self.function_definitions.clone();

        // Process each function definition to extract fetch calls
        for (_, def) in function_defs.iter() {
            // Extract fetch calls based on function type
            let fetch_calls = match &def.node_type {
                FunctionNodeType::ArrowFunction(arrow) => {
                    self.extract_fetch_calls_from_arrow_with_file(arrow, &def.file_path)
                }
                FunctionNodeType::FunctionDeclaration(decl) => {
                    self.extract_fetch_calls_from_function_decl_with_file(decl, &def.file_path)
                }
                FunctionNodeType::FunctionExpression(expr) => {
                    self.extract_fetch_calls_from_function_expr_with_file(expr, &def.file_path)
                }
            };

            // Add the discovered calls
            for mut call in fetch_calls {
                // Set the file path from the function definition if it's empty
                if call.call_file.as_os_str().is_empty() {
                    call.call_file = def.file_path.clone();
                }

                let params = self.extract_params_from_route(&call.route);
                new_calls.push(ApiEndpointDetails {
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

        // Add all newly discovered calls to our collection
        self.calls.extend(new_calls);
    }

    fn byte_offset_to_utf16_offset(source: &str, byte_offset: usize) -> usize {
        source[..byte_offset].encode_utf16().count()
    }

    pub fn generate_type_alias_name(route: &str, method: &str, is_request_type: bool) -> String {
        let prefix = if is_request_type { "Req" } else { "Res" };

        // Sanitize method: Make it PascalCase (e.g., "GET" -> "Get", "post" -> "Post")
        let method_pascal = if method.is_empty() {
            "UnknownMethod".to_string()
        } else {
            let lowercase_method = method.to_lowercase();
            let mut m = lowercase_method.chars();
            match m.next() {
                None => "UnknownMethod".to_string(),
                Some(f) => f.to_uppercase().collect::<String>() + m.as_str(),
            }
        };

        let sanitized_route = Self::sanitize_route_for_dynamic_paths(route);

        format!("{}{}{}", prefix, method_pascal, sanitized_route)
    }

    fn sanitize_route_for_dynamic_paths(route: &str) -> String {
        route
            .split('/')
            .filter(|segment| !segment.is_empty()) // Remove empty segments
            .map(|segment| {
                if segment.starts_with(':') {
                    // Convert :id -> ById, :userId -> ByUserId, :eventId -> ByEventId
                    let param_name = &segment[1..]; // Remove the ':'
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
        func_def_file_path: &PathBuf,
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
            file_path: func_def_file_path.clone(), // Use the function's file path
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
                            let alias = Self::generate_type_alias_name(
                                &endpoint.route,
                                &endpoint.method,
                                true, // is_request_type
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
                            let alias = Self::generate_type_alias_name(
                                &endpoint.route,
                                &endpoint.method,
                                false, // is_request_type = false
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
    ) -> (
        HashMap<(String, String), Json>,
        HashMap<(String, String), Json>,
    ) {
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
    fn update_endpoints_with_resolved_fields(
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

    pub fn analyze_matches(&self) -> (Vec<String>, Vec<String>, Vec<String>) {
        let mut call_issues = Vec::new();
        let mut endpoint_issues = Vec::new();
        let mut env_var_calls = Vec::new();

        // Initialize with all endpoints as potentially orphaned
        let mut orphaned_endpoints: HashSet<(String, String)> = self
            .endpoints
            .iter()
            .map(|api_endpoint_details| {
                (
                    api_endpoint_details.route.clone(),
                    api_endpoint_details.method.clone(),
                )
            })
            .collect();

        // Check each call against endpoints
        for api_call_details in &self.calls {
            // Process the call based on its type
            let path_to_match = if api_call_details.route.contains("ENV_VAR:") {
                // First check if this is a known external API call
                if self.config.is_external_call(&api_call_details.route) {
                    // Skip external API calls entirely
                    continue;
                }
                // Then check if it's a known internal API call
                else if self.config.is_internal_call(&api_call_details.route) {
                    // For internal calls, extract the path portion without the ENV_VAR prefix
                    let mut clean_path = String::new();
                    let segments: Vec<&str> = api_call_details.route.split("ENV_VAR:").collect();

                    // Add the part before any ENV_VAR marker
                    clean_path.push_str(segments[0]);

                    // Process each segment with an ENV_VAR marker
                    for i in 1..segments.len() {
                        let subparts: Vec<&str> = segments[i].splitn(2, ':').collect();
                        if subparts.len() == 2 {
                            clean_path.push_str(subparts[1]);
                        }
                    }

                    clean_path
                }
                // Otherwise it's an unknown env var
                else {
                    // Extract the env var names for the error message
                    let mut env_vars = Vec::new();
                    let segments = api_call_details.route.split("ENV_VAR:");

                    for segment in segments.skip(1) {
                        // Skip the first segment (before any ENV_VAR)
                        let parts: Vec<&str> = segment.splitn(2, ':').collect();
                        if !parts.is_empty() {
                            env_vars.push(parts[0].to_string());
                        }
                    }

                    // Format the error message
                    let env_var_list = env_vars.join(", ");
                    env_var_calls.push(format!(
                        "Environment variable endpoint: {} using env vars [{}] in {}",
                        api_call_details.method, env_var_list, api_call_details.route
                    ));

                    // Skip further processing
                    continue;
                }
            } else {
                // Regular call - use the full route
                api_call_details.route.clone()
            };

            // Try to find a matching endpoint using the determined path
            let endpoint_match = self.find_matching_endpoint(
                &path_to_match,
                &api_call_details.method,
                &self.endpoints,
                &mut orphaned_endpoints,
            );

            // Check if we found a match and if methods are compatible
            match endpoint_match {
                Some((_, endpoint_method)) => {
                    if &api_call_details.method != endpoint_method {
                        call_issues.push(format!(
                            "Method mismatch: {} {} is called but endpoint only supports {}",
                            api_call_details.method, api_call_details.route, endpoint_method
                        ));
                    }
                }
                None => {
                    call_issues.push(format!(
                        "Missing endpoint: No endpoint defined for {} {}",
                        api_call_details.method, api_call_details.route
                    ));
                }
            }
        }

        // After checking all calls, anything left in orphaned_endpoints has no matching call
        for (orphaned_endpoint, orphaned_method) in orphaned_endpoints {
            endpoint_issues.push(format!(
                "Orphaned endpoint: No call matching endpoint {} {}",
                orphaned_method, orphaned_endpoint
            ));
        }

        (call_issues, endpoint_issues, env_var_calls)
    }

    // Helper method to find a matching endpoint using our various matching strategies
    fn find_matching_endpoint<'a>(
        &self,
        route: &str,
        method: &str,
        endpoints: &'a [ApiEndpointDetails],
        orphaned_endpoints: &mut HashSet<(String, String)>,
    ) -> Option<(&'a String, &'a String)> {
        // Safety check
        let router = match &self.endpoint_router {
            Some(r) => r,
            None => return None,
        };

        // Try to match the route with matchit
        match router.at(route) {
            Ok(matched) => {
                // Now we get back a Vec<(String, String)> of route-method pairs
                let route_methods = matched.value;

                // First look for an exact method match
                for (endpoint_route, endpoint_method) in route_methods {
                    if endpoint_method == method {
                        // Find the actual endpoint for this route+method
                        if let Some(endpoint) = endpoints
                            .iter()
                            .find(|ep| &ep.route == endpoint_route && &ep.method == endpoint_method)
                        {
                            // Remove from orphaned endpoints
                            orphaned_endpoints
                                .remove(&(endpoint.route.clone(), endpoint.method.clone()));

                            return Some((&endpoint.route, &endpoint.method));
                        }
                    }
                }

                // If we didn't find an exact method match, return the first endpoint with this route
                // (this is for reporting method mismatches)
                if let Some((endpoint_route, endpoint_method)) = route_methods.first() {
                    if let Some(endpoint) = endpoints
                        .iter()
                        .find(|ep| &ep.route == endpoint_route && &ep.method == endpoint_method)
                    {
                        // Remove from orphaned endpoints
                        orphaned_endpoints
                            .remove(&(endpoint.route.clone(), endpoint.method.clone()));

                        return Some((&endpoint.route, &endpoint.method));
                    }
                }
            }
            Err(_) => {
                // No match found via matchit
            }
        }

        None
    }

    fn normalize_call_route(&self, route: &str) -> String {
        // Remove ENV_VAR prefix if present
        if route.starts_with("ENV_VAR:") {
            // Find the second colon and take everything after it
            if let Some(second_colon) = route.find(':').and_then(|first| {
                route[first + 1..]
                    .find(':')
                    .map(|second| first + 1 + second)
            }) {
                route[second_colon + 1..].to_string()
            } else {
                route.to_string()
            }
        } else {
            route.to_string()
        }
    }

    pub fn compare_calls_to_endpoints(&self) -> Vec<String> {
        let mut issues = Vec::new();

        // Safety check
        let router = match &self.endpoint_router {
            Some(r) => r,
            None => return issues,
        };

        for call in &self.calls {
            let normalized_route = self.normalize_call_route(&call.route);

            match router.at(&normalized_route) {
                Ok(matched) => {
                    // Get the endpoint routes and methods
                    let route_methods = &matched.value;

                    // Look for a method match
                    let matching_endpoint = route_methods
                        .iter()
                        .filter(|(_, endpoint_method)| endpoint_method == &call.method)
                        .find_map(|(endpoint_route, endpoint_method)| {
                            self.endpoints.iter().find(|ep| {
                                &ep.route == endpoint_route && &ep.method == endpoint_method
                            })
                        });

                    if let Some(ep) = matching_endpoint {
                        // Compare request bodies if both exist
                        if let (Some(call_req), Some(ep_req)) =
                            (&call.request_body, &ep.request_body)
                        {
                            let mismatches = self.compare_json_fields(call_req, ep_req, "");
                            for mismatch in mismatches {
                                issues.push(format!(
                                    "Request body mismatch for {} {} -> {}",
                                    call.method, call.route, mismatch
                                ));
                            }
                        }
                    }
                }
                Err(_) => {
                    // No matching endpoint found via matchit
                    // Already reported by analyze_matches()
                }
            }
        }

        issues
    }

    pub fn compare_json_fields(
        &self,
        call_json: &Json,
        endpoint_json: &Json,
        path: &str,
    ) -> Vec<FieldMismatch> {
        let mut mismatches = Vec::new();

        match (call_json, endpoint_json) {
            (Json::Object(call_map), Json::Object(endpoint_map)) => {
                let call_keys: HashSet<_> = call_map.keys().collect();
                let endpoint_keys: HashSet<_> = endpoint_map.keys().collect();

                // Fields required by endpoint but missing in call
                for key in endpoint_keys.difference(&call_keys) {
                    let field_path = if path.is_empty() {
                        key.to_string()
                    } else {
                        format!("{}.{}", path, key)
                    };
                    mismatches.push(FieldMismatch::MissingField(field_path));
                }
                // Fields present in call but not expected by endpoint
                for key in call_keys.difference(&endpoint_keys) {
                    let field_path = if path.is_empty() {
                        key.to_string()
                    } else {
                        format!("{}.{}", path, key)
                    };
                    mismatches.push(FieldMismatch::ExtraField(field_path));
                }
                // Compare common fields recursively
                for key in call_keys.intersection(&endpoint_keys) {
                    let sub_path = if path.is_empty() {
                        key.to_string()
                    } else {
                        format!("{}.{}", path, key)
                    };
                    let sub_mismatches =
                        self.compare_json_fields(&call_map[*key], &endpoint_map[*key], &sub_path);
                    mismatches.extend(sub_mismatches);
                }
            }
            (Json::Array(_), Json::Array(_)) => {
                // You could compare element types here if desired
            }
            (a, b) if std::mem::discriminant(a) != std::mem::discriminant(b) => {
                mismatches.push(FieldMismatch::TypeMismatch(
                    path.to_string(),
                    format!("{:?}", a),
                    format!("{:?}", b),
                ));
            }
            _ => {}
        }

        mismatches
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

    fn build_endpoint_router(&mut self) {
        let mut router = matchit::Router::new();

        // Use a HashMap to collect all endpoints by path before inserting into router
        let mut path_to_endpoints: HashMap<String, Vec<(String, String)>> = HashMap::new();

        for endpoint in &self.endpoints {
            let normalized_route = self.normalize_route_params(&endpoint.route);

            path_to_endpoints
                .entry(normalized_route)
                .or_insert_with(Vec::new)
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
        let repo_suffix = repo_path
            .split('/')
            .filter(|s| !s.is_empty())
            .last()
            .expect("repo_suffix not found");
        let output_path = format!("ts_check/output/{}_types.ts", repo_suffix);

        // Ensure the `ts_check/output` directory exists
        std::fs::create_dir_all("ts_check/output").expect("Failed to create output directory");

        let dependencies = packages.get_dependencies();
        // Serialize dependencies as JSON
        let dependencies_json = serde_json::to_string(dependencies).unwrap();

        // Determine tsconfig path based on repo
        use std::path::Path;
        let tsconfig_path = Path::new(repo_path).join("tsconfig.json");

        println!("Extracting {} types from {}", type_infos.len(), repo_path);

        // Run the extract-type-definitions script with all types at once
        let script_path = std::fs::canonicalize("ts_check/extract-type-definitions.ts")
            .expect("Script not found");
        let ts_config = std::fs::canonicalize(tsconfig_path).expect("tsconfig.json not found");

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

    pub fn get_results(&self) -> ApiAnalysisResult {
        let (call_issues, endpoint_issues, env_var_calls) = self.analyze_matches();
        let mismatches = self.compare_calls_to_endpoints();

        ApiAnalysisResult {
            endpoints: self.endpoints.clone(),
            calls: self.calls.clone(),
            issues: ApiIssues {
                call_issues,
                endpoint_issues,
                env_var_calls,
                mismatches,
            },
        }
    }

    /// Extract repository prefix from endpoint owner information
    fn extract_repo_prefix_from_owner(&self, owner: &Option<OwnerType>) -> String {
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
    fn extract_repo_prefix_from_file_path(
        &self,
        file_path: &PathBuf,
        repo_paths: &[String],
    ) -> String {
        let file_path_str = file_path.to_string_lossy();
        repo_paths
            .iter()
            .find(|repo_path| file_path_str.starts_with(*repo_path))
            .and_then(|repo_path| repo_path.split("/").filter(|s| !s.is_empty()).last())
            .unwrap_or("default")
            .to_string()
    }

    /// Add a TypeReference to the repository type map
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
            let entry = serde_json::json!({
                "filePath": path.to_string(),
                "startPosition": type_ref.start_position,
                "compositeTypeString": type_ref.composite_type_string,
                "alias": type_ref.alias
            });
            repo_type_map.entry(repo_prefix).or_default().push(entry);
        }
    }

    /// Process both request and response types for an ApiEndpointDetails
    fn process_api_detail_types(
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

pub fn analyze_api_consistency(
    visitors: Vec<DependencyVisitor>,
    config: Config,
    packages: Packages,
    cm: Lrc<SourceMap>,
    repo_paths: Vec<String>,
) -> ApiAnalysisResult {
    use std::collections::HashMap;
    // Create and populate our analyzer
    let mut analyzer = Analyzer::new(config, cm.clone());

    // First pass - collect all data from visitors
    for visitor in visitors {
        analyzer.add_visitor_data(visitor);
    }

    let endpoints =
        analyzer.resolve_all_endpoint_paths(&analyzer.endpoints, &analyzer.mounts, &analyzer.apps);
    analyzer.endpoints = endpoints;

    // Build the router after resolving endpoints
    analyzer.build_endpoint_router();

    // Second pass - analyze function definitions for response fields
    let (response_fields, request_fields) = analyzer.resolve_imported_handler_route_fields(
        &analyzer.imported_handlers,
        &analyzer.function_definitions,
    );

    analyzer
        .update_endpoints_with_resolved_fields(response_fields, request_fields)
        .resolve_types_for_endpoints(cm)
        .analyze_functions_for_fetch_calls();

    // Extract types for each repository
    let mut repo_type_map: HashMap<String, Vec<Value>> = HashMap::new();

    // Group type information by repository using endpoint owner information
    for endpoint in &analyzer.endpoints {
        let repo_prefix = analyzer.extract_repo_prefix_from_owner(&endpoint.owner);
        analyzer.process_api_detail_types(endpoint, repo_prefix, &mut repo_type_map);
    }

    // Group type information by repository using call file information
    for call in &analyzer.calls {
        let repo_prefix = analyzer.extract_repo_prefix_from_file_path(&call.file_path, &repo_paths);
        analyzer.process_api_detail_types(call, repo_prefix, &mut repo_type_map);
    }

    // Process types for each repository using the original repo paths
    for repo_path in repo_paths {
        let repo_name = repo_path
            .split("/")
            .filter(|s| !s.is_empty())
            .last()
            .unwrap_or("default");
        if let Some(type_infos) = repo_type_map.get(repo_name) {
            println!(
                "Processing {} types from repository: {}",
                type_infos.len(),
                &repo_path
            );
            analyzer.extract_types_for_repo(&repo_path, type_infos.clone(), &packages);
        }
    }

    analyzer.get_results()
}
