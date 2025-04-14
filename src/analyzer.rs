use crate::visitor::DependencyVisitor;
use std::{collections::HashMap, path::PathBuf};

pub struct ApiAnalysisResult {
    pub endpoints: Vec<(String, String, Vec<String>)>,
    pub calls: Vec<(String, String, Vec<String>)>,
    pub issues: Vec<String>,
}

pub fn analyze_api_consistency(visitors: Vec<DependencyVisitor>) -> ApiAnalysisResult {
    // First pass - collect all data
    let mut all_endpoints = Vec::new();
    let mut all_calls = Vec::new();
    let mut all_imported_handlers = Vec::new();
    let mut all_function_definitions = HashMap::new();

    for visitor in visitors {
        all_endpoints.extend(visitor.endpoints);
        all_calls.extend(visitor.calls);
        all_imported_handlers.extend(visitor.imported_handlers.clone());

        // Merge function definitions
        for (name, def) in visitor.function_definitions {
            all_function_definitions.insert(name, def);
        }
    }

    // Create a combined visitor
    let combined_visitor = DependencyVisitor::new(PathBuf::from("<combined-analysis>"));

    // Second pass - resolve function implementations
    println!("\n=== Second Pass: Analyzing Function Implementations ===");
    let route_fields = combined_visitor
        .analyze_function_definitions(&all_imported_handlers, &all_function_definitions);

    // Print the results of function analysis
    println!("\nResolved Response Fields for Routes:");
    for (route, fields) in &route_fields {
        println!("Route: {} returns fields: {:?}", route, fields);
    }

    // Add the resolved fields to the endpoints
    let mut enhanced_endpoints = all_endpoints.clone();
    for endpoint in &mut enhanced_endpoints {
        let (route, method, _) = endpoint;
        if let Some(fields) = route_fields.get(route) {
            // Replace the fields with the ones from function analysis
            *endpoint = (route.clone(), method.clone(), fields.clone());
        }
    }

    // Third pass - analyze for inconsistencies using the combined data
    let mut analysis_visitor = DependencyVisitor::new(PathBuf::from("<analysis>"));
    analysis_visitor.endpoints = enhanced_endpoints;
    analysis_visitor.calls = all_calls;

    let issues = analysis_visitor.analyze_matches();

    ApiAnalysisResult {
        endpoints: analysis_visitor.endpoints,
        calls: analysis_visitor.calls,
        issues,
    }
}
