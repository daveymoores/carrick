use carrick::{
    agents::{AnalysisResults, CallSiteOrchestrator},
    call_site_extractor::{ArgumentType, CallArgument, CallSite},
    framework_detector::DetectionResult,
    gemini_service::GeminiService,
    mount_graph::MountGraph,
};

/// Test that the multi-agent orchestrator correctly processes call sites in mock mode
#[tokio::test]
async fn test_multi_agent_orchestrator_mock_mode() {
    // Set mock mode
    unsafe {
        std::env::set_var("CARRICK_MOCK_ALL", "1");
    }

    // Create test call sites that simulate what the extractor would find
    let call_sites = vec![
        // app.use('/users', userRouter) - should be classified as RouterMount
        CallSite {
            callee_object: "app".to_string(),
            callee_property: "use".to_string(),
            args: vec![
                CallArgument {
                    arg_type: ArgumentType::StringLiteral,
                    value: Some("/users".to_string()),
                },
                CallArgument {
                    arg_type: ArgumentType::Identifier,
                    value: Some("userRouter".to_string()),
                },
            ],
            definition: Some("const app = express()".to_string()),
            location: "app.ts:10:0".to_string(),
        },
        // router.get('/posts', handler) - should be classified as HttpEndpoint
        CallSite {
            callee_object: "router".to_string(),
            callee_property: "get".to_string(),
            args: vec![
                CallArgument {
                    arg_type: ArgumentType::StringLiteral,
                    value: Some("/posts".to_string()),
                },
                CallArgument {
                    arg_type: ArgumentType::FunctionExpression,
                    value: None,
                },
            ],
            definition: Some("const router = Router()".to_string()),
            location: "routes/api.ts:6:0".to_string(),
        },
        // app.use(express.json()) - should be classified as Middleware
        CallSite {
            callee_object: "app".to_string(),
            callee_property: "use".to_string(),
            args: vec![CallArgument {
                arg_type: ArgumentType::Other,
                value: None,
            }],
            definition: Some("const app = express()".to_string()),
            location: "app.ts:7:0".to_string(),
        },
    ];

    let framework_detection = DetectionResult {
        frameworks: vec!["express".to_string()],
        data_fetchers: vec!["axios".to_string()],
        notes: "Test framework detection".to_string(),
    };

    let gemini_service = GeminiService::new("mock".to_string());
    let orchestrator = CallSiteOrchestrator::new(gemini_service);

    // Run the analysis
    let result = orchestrator
        .analyze_call_sites(&call_sites, &framework_detection)
        .await
        .expect("Analysis should succeed");

    // Verify that triage processed all call sites
    assert_eq!(
        result.triage_stats.total_call_sites, 3,
        "Should process all 3 call sites"
    );

    // In mock mode, we should get mock responses, but the system should still work
    // The exact classification depends on mock response, but we should get non-zero results
    assert!(
        result.triage_stats.endpoints_count > 0
            || result.triage_stats.router_mount_count > 0
            || result.triage_stats.middleware_count > 0,
        "Mock mode should still classify call sites"
    );
}

/// Test that the mount graph correctly builds from analysis results
#[test]
fn test_mount_graph_construction() {
    use carrick::agents::{HttpEndpoint, MountRelationship};

    // Create mock analysis results
    let analysis_results = AnalysisResults {
        endpoints: vec![
            HttpEndpoint {
                method: "GET".to_string(),
                path: "/posts".to_string(),
                handler: "getPosts".to_string(),
                node_name: "apiRouter".to_string(),
                location: "routes/api.ts:6:0".to_string(),
                confidence: 0.9,
                reasoning: "Test endpoint".to_string(),
            },
            HttpEndpoint {
                method: "GET".to_string(),
                path: "/:id".to_string(),
                handler: "getUser".to_string(),
                node_name: "userRouter".to_string(),
                location: "routes/users.ts:6:0".to_string(),
                confidence: 0.9,
                reasoning: "Test endpoint".to_string(),
            },
        ],
        data_fetching_calls: vec![],
        middleware: vec![],
        mount_relationships: vec![
            MountRelationship {
                parent_node: "app".to_string(),
                child_node: "apiRouter".to_string(),
                mount_path: "/api/v1".to_string(),
                location: "app.ts:11:0".to_string(),
                confidence: 0.9,
                reasoning: "Test mount".to_string(),
            },
            MountRelationship {
                parent_node: "app".to_string(),
                child_node: "userRouter".to_string(),
                mount_path: "/users".to_string(),
                location: "app.ts:10:0".to_string(),
                confidence: 0.9,
                reasoning: "Test mount".to_string(),
            },
        ],
        triage_stats: carrick::agents::TriageStats {
            total_call_sites: 0,
            endpoints_count: 2,
            data_fetching_count: 0,
            middleware_count: 0,
            router_mount_count: 2,
            irrelevant_count: 0,
        },
    };

    // Build the mount graph
    let mount_graph = MountGraph::build_from_analysis_results(&analysis_results);

    // Verify nodes were created
    assert_eq!(
        mount_graph.get_nodes().len(),
        3,
        "Should have 3 nodes: app, apiRouter, userRouter"
    );

    // Verify mounts were created
    assert_eq!(
        mount_graph.get_mounts().len(),
        2,
        "Should have 2 mount relationships"
    );

    // Verify endpoints were added
    assert_eq!(
        mount_graph.get_resolved_endpoints().len(),
        2,
        "Should have 2 endpoints"
    );

    // Verify path resolution worked
    let resolved_endpoints = mount_graph.get_resolved_endpoints();

    // Find the apiRouter endpoint
    let api_endpoint = resolved_endpoints
        .iter()
        .find(|e| e.owner == "apiRouter")
        .expect("Should find apiRouter endpoint");
    assert_eq!(
        api_endpoint.full_path, "/api/v1/posts",
        "apiRouter endpoint should have full path /api/v1/posts"
    );

    // Find the userRouter endpoint
    let user_endpoint = resolved_endpoints
        .iter()
        .find(|e| e.owner == "userRouter")
        .expect("Should find userRouter endpoint");
    assert_eq!(
        user_endpoint.full_path, "/users/:id",
        "userRouter endpoint should have full path /users/:id"
    );
}

/// Test that empty call sites are handled gracefully
#[tokio::test]
async fn test_empty_call_sites() {
    unsafe {
        std::env::set_var("CARRICK_MOCK_ALL", "1");
    }

    let framework_detection = DetectionResult {
        frameworks: vec!["express".to_string()],
        data_fetchers: vec![],
        notes: "Test".to_string(),
    };

    let gemini_service = GeminiService::new("mock".to_string());
    let orchestrator = CallSiteOrchestrator::new(gemini_service);

    let result = orchestrator
        .analyze_call_sites(&[], &framework_detection)
        .await
        .expect("Should handle empty call sites");

    assert_eq!(result.triage_stats.total_call_sites, 0);
    assert_eq!(result.endpoints.len(), 0);
    assert_eq!(result.data_fetching_calls.len(), 0);
    assert_eq!(result.mount_relationships.len(), 0);
}
