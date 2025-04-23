use crate::{
    extractor::CoreExtractor,
    visitor::{DependencyVisitor, FunctionDefinition, FunctionNodeType, Json},
};
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;

pub struct ApiIssues {
    pub call_issues: Vec<String>,
    pub endpoint_issues: Vec<String>,
}

impl ApiIssues {
    pub fn is_empty(&self) -> bool {
        self.call_issues.is_empty() && self.endpoint_issues.is_empty()
    }

    pub fn len(&self) -> usize {
        self.call_issues.len() + self.endpoint_issues.len()
    }
}

#[derive(Clone)]
pub struct ApiEndpointDetails {
    pub route: String,
    pub method: String,
    pub params: Vec<String>,
    pub request_body: Option<Json>,
    pub response_body: Option<Json>,
}

pub struct ApiAnalysisResult {
    pub endpoints: Vec<ApiEndpointDetails>,
    pub calls: Vec<ApiEndpointDetails>,
    pub issues: ApiIssues,
}

#[derive(Default)]
struct Analyzer {
    imported_handlers: Vec<(String, String, String)>,
    function_definitions: HashMap<String, FunctionDefinition>,
    endpoints: Vec<ApiEndpointDetails>,
    calls: Vec<ApiEndpointDetails>,
}

impl CoreExtractor for Analyzer {}

impl Analyzer {
    pub fn new() -> Self {
        Default::default()
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
        for (route, method, response) in visitor.endpoints {
            let params = self.extract_params_from_route(&route);
            self.endpoints.push(ApiEndpointDetails {
                route,
                method,
                params,
                response_body: Some(response),
                request_body: None,
            })
        }

        // expected_fields being returned data from all CRUD calls
        for (route, method, expected_fields) in visitor.calls {
            let params = self.extract_params_from_route(&route);
            self.calls.push(ApiEndpointDetails {
                route,
                method,
                params,
                response_body: None,
                request_body: None,
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
            for (route, method) in fetch_calls {
                let params = self.extract_params_from_route(&route);
                new_calls.push(ApiEndpointDetails {
                    route,
                    method,
                    params,
                    request_body: Some(Json::Null),
                    response_body: Some(Json::Null),
                });
            }
        }

        // Add all newly discovered calls to our collection
        self.calls.extend(new_calls);
    }

    // This function analyzes the function definitions and returns a HashMap of route fields.
    pub fn analyze_function_definitions(
        &self,
        imported_handlers: &[(String, String, String)],
        function_definitions: &HashMap<String, FunctionDefinition>,
    ) -> HashMap<String, Json> {
        let mut route_fields = HashMap::new();

        for (route, handler_name, _) in imported_handlers {
            if let Some(func_def) = function_definitions.get(handler_name) {
                let fields = match &func_def.node_type {
                    FunctionNodeType::ArrowFunction(arrow) => self.extract_fields_from_arrow(arrow),
                    FunctionNodeType::FunctionDeclaration(decl) => {
                        self.extract_fields_from_function_decl(decl)
                    }
                    FunctionNodeType::FunctionExpression(expr) => {
                        self.extract_fields_from_function_expr(expr)
                    }
                };

                route_fields.insert(route.clone(), fields);
            }
        }

        route_fields
    }

    pub fn analyze_matches(&self) -> (Vec<String>, Vec<String>) {
        let mut call_issues = Vec::new();
        let mut endpoint_issues = Vec::new();

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

        (call_issues, endpoint_issues)
    }

    pub fn get_results(&self) -> ApiAnalysisResult {
        let (call_issues, endpoint_issues) = self.analyze_matches();

        ApiAnalysisResult {
            endpoints: self.endpoints.clone(),
            calls: self.calls.clone(),
            issues: ApiIssues {
                call_issues,
                endpoint_issues,
            },
        }
    }
}

pub fn analyze_api_consistency(visitors: Vec<DependencyVisitor>) -> ApiAnalysisResult {
    // Create and populate our analyzer
    let mut analyzer = Analyzer::new();

    // First pass - collect all data from visitors
    for visitor in visitors {
        analyzer.add_visitor_data(visitor);
    }
    // Second pass - analyze function definitions for response fields
    println!("\n=== Second Pass: Analyzing Function Implementations ===");
    let route_fields = analyzer
        .analyze_function_definitions(&analyzer.imported_handlers, &analyzer.function_definitions);

    // Print the results of function analysis
    println!("\nResolved Response Fields for Routes:");
    for (route, fields) in &route_fields {
        println!("Route: {} returns: {}", route, json!(fields));
    }

    // Third pass - look for fetch calls inside functions
    println!("\n=== Third Pass: Analyzing Fetch Calls in Functions ===");

    analyzer.analyze_functions_for_fetch_calls();

    // Get and return the final analysis results
    analyzer.get_results()
}
