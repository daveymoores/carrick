use carrick::analyzer::{Analyzer, ApiEndpointDetails};
use carrick::config::Config;
use carrick::visitor::OwnerType;
use std::path::PathBuf;
use swc_common::{SourceMap, sync::Lrc};

/// Helper function to create a test endpoint
fn create_endpoint(route: &str, method: &str, file_path: &str) -> ApiEndpointDetails {
    ApiEndpointDetails {
        owner: Some(OwnerType::App("app".to_string())),
        route: route.to_string(),
        method: method.to_string(),
        params: vec![],
        request_body: None,
        response_body: None,
        handler_name: None,
        request_type: None,
        response_type: None,
        file_path: PathBuf::from(file_path),
    }
}

/// Helper function to create a test API call
fn create_call(route: &str, method: &str, file_path: &str) -> ApiEndpointDetails {
    ApiEndpointDetails {
        owner: None, // Calls don't have owners
        route: route.to_string(),
        method: method.to_string(),
        params: vec![],
        request_body: None,
        response_body: None,
        handler_name: None,
        request_type: None,
        response_type: None,
        file_path: PathBuf::from(file_path),
    }
}

#[tokio::test]
async fn test_matching_endpoint_and_call() {
    // Given: an analyzer with a matching endpoint and call
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Producer defines GET /api/users
    let endpoint = create_endpoint("/api/users", "GET", "server.ts");
    analyzer.endpoints.push(endpoint);

    // Consumer calls GET /api/users
    let call = create_call("/api/users", "GET", "client.ts");
    analyzer.calls.push(call);

    analyzer.build_endpoint_router();

    // When: analyze matches
    let (call_issues, endpoint_issues, _env_var_calls) = analyzer.analyze_matches();

    // Then: no issues should be found
    assert_eq!(
        call_issues.len(),
        0,
        "Expected no call issues but found: {:?}",
        call_issues
    );
    assert_eq!(
        endpoint_issues.len(),
        0,
        "Expected no endpoint issues but found: {:?}",
        endpoint_issues
    );
}

#[tokio::test]
async fn test_missing_endpoint() {
    // Given: an analyzer with a call to a non-existent endpoint
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // No endpoint defined

    // Consumer calls GET /api/users
    let call = create_call("/api/users", "GET", "client.ts");
    analyzer.calls.push(call);

    analyzer.build_endpoint_router();

    // When: analyze matches
    let (call_issues, _endpoint_issues, _env_var_calls) = analyzer.analyze_matches();

    // Then: should detect missing endpoint
    assert_eq!(call_issues.len(), 1);
    assert!(
        call_issues[0].contains("Missing endpoint"),
        "Expected 'Missing endpoint' but got: {}",
        call_issues[0]
    );
    assert!(
        call_issues[0].contains("GET /api/users"),
        "Expected 'GET /api/users' but got: {}",
        call_issues[0]
    );
}

#[tokio::test]
async fn test_method_mismatch() {
    // Given: an analyzer with endpoint and call with different methods
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Producer defines GET /api/users
    let endpoint = create_endpoint("/api/users", "GET", "server.ts");
    analyzer.endpoints.push(endpoint);

    // Consumer calls POST /api/users
    let call = create_call("/api/users", "POST", "client.ts");
    analyzer.calls.push(call);

    analyzer.build_endpoint_router();

    // When: analyze matches
    let (call_issues, _endpoint_issues, _env_var_calls) = analyzer.analyze_matches();

    // Then: should detect method mismatch
    assert_eq!(call_issues.len(), 1);
    assert!(
        call_issues[0].contains("Method mismatch"),
        "Expected 'Method mismatch' but got: {}",
        call_issues[0]
    );
    assert!(
        call_issues[0].contains("POST /api/users") && call_issues[0].contains("GET"),
        "Expected POST/GET mismatch message but got: {}",
        call_issues[0]
    );
}

#[tokio::test]
async fn test_orphaned_endpoint() {
    // Given: an analyzer with an endpoint that has no calls
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Producer defines GET /api/users
    let endpoint = create_endpoint("/api/users", "GET", "server.ts");
    analyzer.endpoints.push(endpoint);

    // No calls to this endpoint

    analyzer.build_endpoint_router();

    // When: analyze matches
    let (_call_issues, endpoint_issues, _env_var_calls) = analyzer.analyze_matches();

    // Then: should detect orphaned endpoint
    assert_eq!(endpoint_issues.len(), 1);
    assert!(
        endpoint_issues[0].contains("Orphaned endpoint"),
        "Expected 'Orphaned endpoint' but got: {}",
        endpoint_issues[0]
    );
    assert!(
        endpoint_issues[0].contains("GET /api/users"),
        "Expected 'GET /api/users' but got: {}",
        endpoint_issues[0]
    );
}

#[tokio::test]
async fn test_path_parameter_matching() {
    // Given: an analyzer with endpoint using :id and call using :userId
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Producer defines GET /api/users/:id
    let endpoint = create_endpoint("/api/users/:id", "GET", "server.ts");
    analyzer.endpoints.push(endpoint);

    // Consumer calls GET /api/users/:userId (different param name)
    let call = create_call("/api/users/:userId", "GET", "client.ts");
    analyzer.calls.push(call);

    analyzer.build_endpoint_router();

    // When: analyze matches
    let (call_issues, endpoint_issues, _env_var_calls) = analyzer.analyze_matches();

    // Then: should match despite different param names
    assert_eq!(
        call_issues.len(),
        0,
        "Expected no call issues (params should normalize) but found: {:?}",
        call_issues
    );
    assert_eq!(
        endpoint_issues.len(),
        0,
        "Expected no endpoint issues but found: {:?}",
        endpoint_issues
    );
}

#[tokio::test]
async fn test_multiple_methods_on_same_path() {
    // Given: an analyzer with multiple methods on the same path
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Producer defines GET and POST /api/users
    let endpoint_get = create_endpoint("/api/users", "GET", "server.ts");
    let endpoint_post = create_endpoint("/api/users", "POST", "server.ts");
    analyzer.endpoints.push(endpoint_get);
    analyzer.endpoints.push(endpoint_post);

    // Consumer calls both GET and POST
    let call_get = create_call("/api/users", "GET", "client.ts");
    let call_post = create_call("/api/users", "POST", "client.ts");
    analyzer.calls.push(call_get);
    analyzer.calls.push(call_post);

    analyzer.build_endpoint_router();

    // When: analyze matches
    let (call_issues, endpoint_issues, _env_var_calls) = analyzer.analyze_matches();

    // Then: both should match correctly
    assert_eq!(
        call_issues.len(),
        0,
        "Expected no call issues but found: {:?}",
        call_issues
    );
    assert_eq!(
        endpoint_issues.len(),
        0,
        "Expected no endpoint issues but found: {:?}",
        endpoint_issues
    );
}

#[tokio::test]
async fn test_multiple_calls_to_same_endpoint() {
    // Given: an analyzer with multiple calls to the same endpoint
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Producer defines GET /api/users
    let endpoint = create_endpoint("/api/users", "GET", "server.ts");
    analyzer.endpoints.push(endpoint);

    // Consumer calls GET /api/users multiple times from different files
    let call1 = create_call("/api/users", "GET", "client1.ts");
    let call2 = create_call("/api/users", "GET", "client2.ts");
    let call3 = create_call("/api/users", "GET", "client3.ts");
    analyzer.calls.push(call1);
    analyzer.calls.push(call2);
    analyzer.calls.push(call3);

    analyzer.build_endpoint_router();

    // When: analyze matches
    let (call_issues, endpoint_issues, _env_var_calls) = analyzer.analyze_matches();

    // Then: all calls should match
    assert_eq!(
        call_issues.len(),
        0,
        "Expected no call issues but found: {:?}",
        call_issues
    );
    assert_eq!(
        endpoint_issues.len(),
        0,
        "Expected no endpoint issues but found: {:?}",
        endpoint_issues
    );
}

#[tokio::test]
async fn test_complex_scenario_with_mixed_matches_and_mismatches() {
    // Given: a complex scenario with various matches and mismatches
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Producer defines:
    // - GET /api/users
    // - POST /api/users
    // - GET /api/users/:id
    // - GET /api/products
    analyzer
        .endpoints
        .push(create_endpoint("/api/users", "GET", "server.ts"));
    analyzer
        .endpoints
        .push(create_endpoint("/api/users", "POST", "server.ts"));
    analyzer
        .endpoints
        .push(create_endpoint("/api/users/:id", "GET", "server.ts"));
    analyzer
        .endpoints
        .push(create_endpoint("/api/products", "GET", "server.ts"));

    // Consumer calls:
    // - GET /api/users (✓ matches)
    // - POST /api/users (✓ matches)
    // - DELETE /api/users/:id (✗ method mismatch - only GET defined)
    // - GET /api/orders (✗ missing endpoint)
    // Products endpoint is orphaned (no call)
    analyzer
        .calls
        .push(create_call("/api/users", "GET", "client.ts"));
    analyzer
        .calls
        .push(create_call("/api/users", "POST", "client.ts"));
    analyzer
        .calls
        .push(create_call("/api/users/:id", "DELETE", "client.ts"));
    analyzer
        .calls
        .push(create_call("/api/orders", "GET", "client.ts"));

    analyzer.build_endpoint_router();

    // When: analyze matches
    let (call_issues, endpoint_issues, _env_var_calls) = analyzer.analyze_matches();

    // Then: should detect 2 call issues and 1 orphaned endpoint
    assert_eq!(
        call_issues.len(),
        2,
        "Expected 2 call issues but found: {:?}",
        call_issues
    );
    assert_eq!(
        endpoint_issues.len(),
        1,
        "Expected 1 orphaned endpoint but found: {:?}",
        endpoint_issues
    );

    // Verify specific issues
    let call_issues_str = call_issues.join("\n");
    assert!(
        call_issues_str.contains("DELETE") && call_issues_str.contains("/api/users/:id"),
        "Expected DELETE /api/users/:id issue"
    );
    assert!(
        call_issues_str.contains("/api/orders"),
        "Expected /api/orders missing endpoint issue"
    );

    let endpoint_issues_str = endpoint_issues.join("\n");
    assert!(
        endpoint_issues_str.contains("/api/products"),
        "Expected /api/products orphaned endpoint"
    );
}

#[tokio::test]
async fn test_deduplication_of_calls() {
    // Given: an analyzer with duplicate calls from the same file
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Producer defines GET /api/users
    let endpoint = create_endpoint("/api/users", "GET", "server.ts");
    analyzer.endpoints.push(endpoint);

    // Consumer makes the same call multiple times from the same file (should deduplicate)
    for _ in 0..5 {
        let call = create_call("/api/users", "GET", "client.ts");
        analyzer.calls.push(call);
    }

    analyzer.build_endpoint_router();

    // When: analyze matches
    let (call_issues, endpoint_issues, _env_var_calls) = analyzer.analyze_matches();

    // Then: should match and not report duplicates
    assert_eq!(
        call_issues.len(),
        0,
        "Expected no call issues (duplicates should be deduplicated) but found: {:?}",
        call_issues
    );
    assert_eq!(
        endpoint_issues.len(),
        0,
        "Expected no endpoint issues but found: {:?}",
        endpoint_issues
    );
}

#[tokio::test]
async fn test_rest_api_crud_operations() {
    // Given: a typical REST API setup
    let config = Config::default();
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(config, cm);

    // Producer defines full CRUD operations
    analyzer
        .endpoints
        .push(create_endpoint("/api/users", "GET", "server.ts")); // List
    analyzer
        .endpoints
        .push(create_endpoint("/api/users", "POST", "server.ts")); // Create
    analyzer
        .endpoints
        .push(create_endpoint("/api/users/:id", "GET", "server.ts")); // Read
    analyzer
        .endpoints
        .push(create_endpoint("/api/users/:id", "PUT", "server.ts")); // Update
    analyzer
        .endpoints
        .push(create_endpoint("/api/users/:id", "DELETE", "server.ts")); // Delete

    // Consumer uses all operations
    analyzer
        .calls
        .push(create_call("/api/users", "GET", "client.ts"));
    analyzer
        .calls
        .push(create_call("/api/users", "POST", "client.ts"));
    analyzer
        .calls
        .push(create_call("/api/users/:id", "GET", "client.ts"));
    analyzer
        .calls
        .push(create_call("/api/users/:id", "PUT", "client.ts"));
    analyzer
        .calls
        .push(create_call("/api/users/:id", "DELETE", "client.ts"));

    analyzer.build_endpoint_router();

    // When: analyze matches
    let (call_issues, endpoint_issues, _env_var_calls) = analyzer.analyze_matches();

    // Then: all should match perfectly
    assert_eq!(
        call_issues.len(),
        0,
        "Expected no call issues in full REST API but found: {:?}",
        call_issues
    );
    assert_eq!(
        endpoint_issues.len(),
        0,
        "Expected no endpoint issues in full REST API but found: {:?}",
        endpoint_issues
    );
}
