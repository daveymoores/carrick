use crate::visitor::DependencyVisitor;

pub struct ApiAnalysisResult {
    pub endpoints: Vec<(String, String, Vec<String>)>,
    pub calls: Vec<(String, String, Vec<String>)>,
    pub issues: Vec<String>,
}

pub fn analyze_api_consistency(visitors: Vec<DependencyVisitor>) -> ApiAnalysisResult {
    let mut all_endpoints = Vec::new();
    let mut all_calls = Vec::new();

    // Collect all endpoints and calls from all files
    for visitor in visitors {
        all_endpoints.extend(visitor.endpoints);
        all_calls.extend(visitor.calls);
    }

    // Create a combined visitor for analysis
    let mut combined_visitor = DependencyVisitor::new();
    combined_visitor.endpoints = all_endpoints;
    combined_visitor.calls = all_calls;

    // Analyze for inconsistencies
    let issues = combined_visitor.analyze_matches();

    ApiAnalysisResult {
        endpoints: combined_visitor.endpoints,
        calls: combined_visitor.calls,
        issues,
    }
}
