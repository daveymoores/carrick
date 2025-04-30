use crate::{
    config::Config,
    extractor::CoreExtractor,
    visitor::{DependencyVisitor, FunctionDefinition, FunctionNodeType, Json},
};
use core::fmt;
use regex::Regex;
use std::collections::HashMap;
use std::collections::HashSet;

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

#[derive(Default)]
struct Analyzer {
    // <Route, http_method, handler_name, source>
    imported_handlers: Vec<(String, String, String, String)>,
    function_definitions: HashMap<String, FunctionDefinition>,
    endpoints: Vec<ApiEndpointDetails>,
    calls: Vec<ApiEndpointDetails>,
    config: Config,
}

#[derive(Debug)]
pub enum FieldMismatch {
    MissingField(String),
    ExtraField(String),
    TypeMismatch(String, String, String), // (path, call_type, endpoint_type)
}

impl CoreExtractor for Analyzer {}

impl Analyzer {
    pub fn new(config: Config) -> Self {
        Analyzer {
            config,
            ..Default::default()
        }
    }

    fn normalize_route(&self, route: &str) -> String {
        let mut normalized = route.to_string();

        // Remove trailing slashes
        while normalized.ends_with('/') && normalized.len() > 1 {
            normalized.pop();
        }

        // Ensure leading slash
        if !normalized.starts_with('/') {
            normalized = format!("/{}", normalized);
        }

        // Create a regex-based matcher for parameters
        let param_pattern = regex::Regex::new(r":([\w\d]+)").unwrap();

        // Replace all parameters with a common placeholder for comparison
        normalized = param_pattern.replace_all(&normalized, ":param").to_string();

        normalized
    }

    // Strip parameters for prefix matching
    fn strip_params(&self, route: &str) -> String {
        let param_pattern = regex::Regex::new(r"/:[\w\d]+").unwrap();
        let base_route = param_pattern.replace_all(route, "").to_string();
        if base_route.is_empty() {
            "/".to_string()
        } else {
            base_route
        }
    }

    // Convert a route with parameters to a regex pattern
    fn route_to_regex(&self, route: &str) -> Regex {
        let pattern = route
            .split('/')
            .map(|segment| {
                if segment.starts_with(':') {
                    "([^/]+)".to_string() // Match any character except /
                } else if !segment.is_empty() {
                    regex::escape(segment) // Escape other segments
                } else {
                    "".to_string()
                }
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<String>>()
            .join("/");

        // Ensure pattern matches the whole path with proper anchoring
        let re_str = format!("^/?{}/?$", pattern);
        Regex::new(&re_str).unwrap_or_else(|_| Regex::new("^/$").unwrap())
    }

    pub fn add_visitor_data(&mut self, visitor: DependencyVisitor) {
        for (route, method, response, request) in visitor.endpoints {
            let params = self.extract_params_from_route(&route);
            self.endpoints.push(ApiEndpointDetails {
                route,
                method,
                params,
                response_body: Some(response),
                request_body: request, // Now we're using the extracted request body
            });
        }

        // expected_fields being returned data from all CRUD calls
        for (route, method, response, request) in visitor.calls {
            let params = self.extract_params_from_route(&route);
            self.calls.push(ApiEndpointDetails {
                route,
                method,
                params,
                response_body: Some(response),
                request_body: request,
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
                    self.extract_fetch_calls_from_arrow(arrow)
                }
                FunctionNodeType::FunctionDeclaration(decl) => {
                    self.extract_fetch_calls_from_function_decl(decl)
                }
                FunctionNodeType::FunctionExpression(expr) => {
                    self.extract_fetch_calls_from_function_expr(expr)
                }
            };

            // Add the discovered calls
            for (route, method, request_body) in fetch_calls {
                let params = self.extract_params_from_route(&route);
                new_calls.push(ApiEndpointDetails {
                    route,
                    method,
                    params,
                    request_body,
                    response_body: Some(Json::Null),
                });
            }
        }

        // Add all newly discovered calls to our collection
        self.calls.extend(new_calls);
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

        // Initialize with all endpoints as potentially orphaned - use cloned strings for owned values
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
            // Check if this is an env var call
            if api_call_details.route.starts_with("ENV_VAR:") {
                let parts: Vec<&str> = api_call_details.route.split(':').collect();
                if parts.len() >= 3 {
                    let env_var = parts[1];
                    let path = parts[2..].join(":");

                    if self.config.is_external_call(&api_call_details.route) {
                        // This is a known external API call - don't report as an issue
                        continue;
                    } else if self.config.is_internal_call(&api_call_details.route) {
                        // This is a known internal service call
                        // For internal calls, we might still need to check if the endpoint exists
                        // but we'd need more context about the service structure
                        continue;
                    } else {
                        // This is an unknown env var
                        env_var_calls.push(format!(
                            "Environment variable endpoint: {} ${{process.env.{}}}{}",
                            api_call_details.method, env_var, path
                        ));
                    }

                    // Skip the rest of the loop for env var calls
                    continue;
                }
            }

            let normalized_call = self.normalize_route(&api_call_details.route);
            // Try to find a matching endpoint using various strategies
            let mut endpoint_match = None;

            for api_endpoint_details in &self.endpoints {
                // Strategy 1: Direct match after normalization
                let normalized_endpoint = self.normalize_route(&api_endpoint_details.route);
                if normalized_call == normalized_endpoint {
                    endpoint_match =
                        Some((&api_endpoint_details.route, &api_endpoint_details.method));
                    orphaned_endpoints.remove(&(
                        api_endpoint_details.route.clone(),
                        api_endpoint_details.method.clone(),
                    ));
                    break;
                }

                // Strategy 2: Parameter-aware regex matching
                if api_endpoint_details.route.contains(':') {
                    let regex = self.route_to_regex(&api_endpoint_details.route);
                    if regex.is_match(&api_call_details.route) {
                        endpoint_match =
                            Some((&api_endpoint_details.route, &api_endpoint_details.method));
                        orphaned_endpoints.remove(&(
                            api_endpoint_details.route.clone(),
                            api_endpoint_details.method.clone(),
                        ));
                        break;
                    }
                }

                // Strategy 3: Check if it's a sub-route
                if api_call_details
                    .route
                    .starts_with(&self.strip_params(&api_endpoint_details.route))
                    && !api_endpoint_details.route.contains(':')
                {
                    endpoint_match =
                        Some((&api_endpoint_details.route, &api_endpoint_details.method));
                    orphaned_endpoints.remove(&(
                        api_endpoint_details.route.clone(),
                        api_endpoint_details.method.clone(),
                    ));
                    break;
                }
            }

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

    pub fn compare_calls_to_endpoints(&self) -> Vec<String> {
        let mut issues = Vec::new();

        for call in &self.calls {
            // Find matching endpoint by route and method
            let endpoint = self.endpoints.iter().find(|ep| {
                self.normalize_route(&ep.route) == self.normalize_route(&call.route)
                    && ep.method == call.method
            });

            if let Some(ep) = endpoint {
                // Compare request bodies if both exist
                if let (Some(call_req), Some(ep_req)) = (&call.request_body, &ep.request_body) {
                    let mismatches = self.compare_json_fields(call_req, ep_req, "");
                    for mismatch in mismatches {
                        issues.push(format!(
                            "Request body mismatch for {} {} -> {}",
                            call.method, call.route, mismatch
                        ));
                    }
                }
                // Optionally, compare response bodies
                // if let (Some(call_resp), Some(ep_resp)) = (&call.response_body, &ep.response_body) {
                //     let mismatches = compare_json_fields(call_resp, ep_resp, "");
                //     for mismatch in mismatches {
                //         issues.push(format!(
                //             "Response body mismatch for {} {}: {:?}",
                //             call.method, call.route, mismatch
                //         ));
                //     }
                // }
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
}

pub fn analyze_api_consistency(
    visitors: Vec<DependencyVisitor>,
    config: Config,
) -> ApiAnalysisResult {
    // Create and populate our analyzer
    let mut analyzer = Analyzer::new(config);

    // First pass - collect all data from visitors
    for visitor in visitors {
        analyzer.add_visitor_data(visitor);
    }

    // Second pass - analyze function definitions for response fields
    let (response_fields, request_fields) = analyzer.resolve_imported_handler_route_fields(
        &analyzer.imported_handlers,
        &analyzer.function_definitions,
    );

    analyzer
        .update_endpoints_with_resolved_fields(response_fields, request_fields)
        .analyze_functions_for_fetch_calls();

    analyzer.get_results()
}
