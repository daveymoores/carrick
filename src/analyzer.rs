use crate::{
    extractor::CoreExtractor,
    visitor::{DependencyVisitor, FunctionDefinition, FunctionNodeType, Json},
};
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;

pub struct ApiAnalysisResult {
    pub endpoints: Vec<(String, String, Json)>,
    pub calls: Vec<(String, String, Json)>,
    pub issues: Vec<String>,
}

#[derive(Default)]
struct Analyzer {
    imported_handlers: Vec<(String, String, String)>,
    function_definitions: HashMap<String, FunctionDefinition>,
    endpoints: Vec<(String, String, Json)>,
    calls: Vec<(String, String, Json)>,
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

        normalized
    }

    pub fn add_visitor_data(&mut self, visitor: DependencyVisitor) {
        self.endpoints.extend(visitor.endpoints);
        self.calls.extend(visitor.calls);
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
                new_calls.push((route, method, Json::Null));
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

    pub fn analyze_matches(&self) -> Vec<String> {
        let mut issues = Vec::new();

        // Create a map of endpoints for efficient lookups
        let mut endpoint_map: HashMap<String, Vec<String>> = HashMap::new();
        for (route, method, _) in &self.endpoints {
            let normalized_route = self.normalize_route(route);
            endpoint_map
                .entry(normalized_route)
                .or_insert_with(Vec::new)
                .push(method.clone());
        }

        // Check each call against endpoints
        for (route, method, _) in &self.calls {
            let normalized_route = self.normalize_route(route);

            // Check if route exists
            match endpoint_map.get(&normalized_route) {
                Some(allowed_methods) => {
                    // Check if method is allowed
                    if !allowed_methods.contains(method) {
                        issues.push(format!(
                            "Method mismatch: {} {} is called but endpoint only supports methods: {:?}",
                            method, route, allowed_methods
                        ));
                    }
                }
                None => {
                    // Try to find if it's a sub-route or has a parent
                    let mut found = false;

                    // Check for API base paths (simple approach)
                    for (endpoint_route, _, _) in &self.endpoints {
                        let norm_endpoint = self.normalize_route(endpoint_route);
                        if normalized_route.starts_with(&norm_endpoint) {
                            found = true;
                            break;
                        }

                        // Check for route parameters like '/users/:id'
                        if endpoint_route.contains(':') {
                            // Convert route with params to regex pattern
                            // Replace :param with a regex capture group that matches everything except slashes
                            let pattern = norm_endpoint
                                .split('/')
                                .map(|segment| {
                                    if segment.starts_with(':') {
                                        "([^/]+)".to_string() // Match any character except /
                                    } else {
                                        regex::escape(segment) // Escape other segments for regex safety
                                    }
                                })
                                .collect::<Vec<String>>()
                                .join("/");

                            // Ensure pattern matches the whole path
                            let re_str = format!("^{}$", pattern);

                            // Create regex and check for match
                            if let Ok(regex) = Regex::new(&re_str) {
                                if regex.is_match(&normalized_route) {
                                    found = true;
                                    break;
                                }
                            }
                        }
                    }

                    if !found {
                        issues.push(format!(
                            "Missing endpoint: No endpoint defined for {} {}",
                            method, route
                        ));
                    }
                }
            }
        }

        issues
    }

    pub fn get_results(&self) -> ApiAnalysisResult {
        ApiAnalysisResult {
            endpoints: self.endpoints.clone(),
            calls: self.calls.clone(),
            issues: self.analyze_matches(),
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
