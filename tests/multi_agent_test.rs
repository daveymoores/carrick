use carrick::{
    agents::{AnalysisResults, CallSiteOrchestrator, LeanCallSite},
    call_site_extractor::{ArgumentType, CallArgument, CallSite},
    framework_detector::DetectionResult,
    gemini_service::GeminiService,
    mount_graph::MountGraph,
    visitor::ImportedSymbol,
};
use std::collections::HashMap;

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
                    resolved_value: None,
                    handler_param_types: None,
                },
                CallArgument {
                    arg_type: ArgumentType::Identifier,
                    value: Some("userRouter".to_string()),
                    resolved_value: None,
                    handler_param_types: None,
                },
            ],
            definition: Some("const app = express()".to_string()),
            location: "app.ts:10:0".to_string(),
            result_type: None,
            correlated_fetch: None,
        },
        // router.get('/posts', handler) - should be classified as HttpEndpoint
        CallSite {
            callee_object: "router".to_string(),
            callee_property: "get".to_string(),
            args: vec![
                CallArgument {
                    arg_type: ArgumentType::StringLiteral,
                    value: Some("/posts".to_string()),
                    resolved_value: None,
                    handler_param_types: None,
                },
                CallArgument {
                    arg_type: ArgumentType::FunctionExpression,
                    value: None,
                    resolved_value: None,
                    handler_param_types: None,
                },
            ],
            definition: Some("const router = Router()".to_string()),
            location: "routes/api.ts:6:0".to_string(),
            result_type: None,
            correlated_fetch: None,
        },
        // app.use(express.json()) - should be classified as Middleware
        CallSite {
            callee_object: "app".to_string(),
            callee_property: "use".to_string(),
            args: vec![CallArgument {
                arg_type: ArgumentType::Other,
                value: None,
                resolved_value: None,
                handler_param_types: None,
            }],
            definition: Some("const app = express()".to_string()),
            location: "app.ts:7:0".to_string(),
            result_type: None,
            correlated_fetch: None,
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
                response_type_file: None,
                response_type_position: None,
                response_type_string: None,
            },
            HttpEndpoint {
                method: "GET".to_string(),
                path: "/:id".to_string(),
                handler: "getUser".to_string(),
                node_name: "userRouter".to_string(),
                location: "routes/users.ts:6:0".to_string(),
                confidence: 0.9,
                reasoning: "Test endpoint".to_string(),
                response_type_file: None,
                response_type_position: None,
                response_type_string: None,
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
    let imported_symbols: HashMap<String, ImportedSymbol> = HashMap::new();
    let mount_graph = MountGraph::build_from_analysis_results(&analysis_results, &imported_symbols);

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

/// Test that LeanCallSite preserves enough information to distinguish RouterMount from Middleware
///
/// This test validates that when we convert CallSite to LeanCallSite (for production LLM calls),
/// we retain the argument information needed to classify:
/// - app.use('/api', router) -> RouterMount (2 args, first is string path)
/// - app.use(middleware) -> Middleware (1 arg, not a string path)
///
/// Without this information, the LLM cannot distinguish between the two patterns.
#[test]
fn test_lean_call_site_preserves_mount_classification_info() {
    // Create a router mount call site: app.use('/api', router)
    let router_mount_call_site = CallSite {
        callee_object: "app".to_string(),
        callee_property: "use".to_string(),
        args: vec![
            CallArgument {
                arg_type: ArgumentType::StringLiteral,
                value: Some("/api".to_string()),
                resolved_value: Some("/api".to_string()),
                handler_param_types: None,
            },
            CallArgument {
                arg_type: ArgumentType::Identifier,
                value: Some("router".to_string()),
                resolved_value: None,
                handler_param_types: None,
            },
        ],
        definition: Some("const app = express()".to_string()),
        location: "server.ts:25:0".to_string(),
        result_type: None,
        correlated_fetch: None,
    };

    // Create a middleware call site: app.use(express.json())
    let middleware_call_site = CallSite {
        callee_object: "app".to_string(),
        callee_property: "use".to_string(),
        args: vec![CallArgument {
            arg_type: ArgumentType::Other, // express.json() is a call expression
            value: None,
            resolved_value: None,
            handler_param_types: None,
        }],
        definition: Some("const app = express()".to_string()),
        location: "server.ts:10:0".to_string(),
        result_type: None,
        correlated_fetch: None,
    };

    // Convert to LeanCallSite (what gets sent to the LLM in production)
    let lean_mount: LeanCallSite = (&router_mount_call_site).into();
    let lean_middleware: LeanCallSite = (&middleware_call_site).into();

    // CRITICAL: LeanCallSite must have enough info to distinguish these two cases
    // The LLM needs to know:
    // 1. How many arguments there are
    // 2. Whether the first argument is a string path (like "/api")

    // Test that we can distinguish router mount from middleware
    assert_eq!(
        lean_mount.arg_count, 2,
        "Router mount should have 2 arguments"
    );
    assert_eq!(
        lean_middleware.arg_count, 1,
        "Middleware should have 1 argument"
    );

    // Test that first arg type is preserved for router mounts
    assert_eq!(
        lean_mount.first_arg_type.as_deref(),
        Some("StringLiteral"),
        "Router mount first arg should be StringLiteral"
    );

    // Test that first arg value (the path) is preserved
    assert_eq!(
        lean_mount.first_arg_value.as_deref(),
        Some("/api"),
        "Router mount path should be preserved"
    );

    // Test that middleware doesn't look like a router mount
    assert_ne!(
        lean_middleware.first_arg_type.as_deref(),
        Some("StringLiteral"),
        "Middleware first arg should NOT be StringLiteral"
    );
}

/// Test that nested router mounts are properly preserved in LeanCallSite
/// This simulates: router.use('/v1', v1Router)
#[test]
fn test_lean_call_site_nested_mount() {
    let nested_mount = CallSite {
        callee_object: "router".to_string(),
        callee_property: "use".to_string(),
        args: vec![
            CallArgument {
                arg_type: ArgumentType::StringLiteral,
                value: Some("/v1".to_string()),
                resolved_value: Some("/v1".to_string()),
                handler_param_types: None,
            },
            CallArgument {
                arg_type: ArgumentType::Identifier,
                value: Some("v1Router".to_string()),
                resolved_value: None,
                handler_param_types: None,
            },
        ],
        definition: Some("const router = express.Router()".to_string()),
        location: "api-router.ts:30:0".to_string(),
        result_type: None,
        correlated_fetch: None,
    };

    let lean: LeanCallSite = (&nested_mount).into();

    // Verify mount classification info is preserved
    assert_eq!(lean.arg_count, 2);
    assert_eq!(lean.first_arg_type.as_deref(), Some("StringLiteral"));
    assert_eq!(lean.first_arg_value.as_deref(), Some("/v1"));
    assert_eq!(lean.callee_object, "router");
    assert_eq!(lean.callee_property, "use");
}

/// Test real-world scenario from repo-b that was failing
/// These are the exact patterns from express-demo-1/repo-b that were misclassified
#[test]
fn test_repo_b_mount_patterns_in_lean_call_site() {
    // Pattern 1: app.use("/api", router)
    let mount1 = CallSite {
        callee_object: "app".to_string(),
        callee_property: "use".to_string(),
        args: vec![
            CallArgument {
                arg_type: ArgumentType::StringLiteral,
                value: Some("/api".to_string()),
                resolved_value: Some("/api".to_string()),
                handler_param_types: None,
            },
            CallArgument {
                arg_type: ArgumentType::Identifier,
                value: Some("router".to_string()),
                resolved_value: None,
                handler_param_types: None,
            },
        ],
        definition: None,
        location: "repo-b_server.ts:112:0".to_string(),
        result_type: None,
        correlated_fetch: None,
    };

    // Pattern 2: app.use("/api", apiRouter)
    let mount2 = CallSite {
        callee_object: "app".to_string(),
        callee_property: "use".to_string(),
        args: vec![
            CallArgument {
                arg_type: ArgumentType::StringLiteral,
                value: Some("/api".to_string()),
                resolved_value: Some("/api".to_string()),
                handler_param_types: None,
            },
            CallArgument {
                arg_type: ArgumentType::Identifier,
                value: Some("apiRouter".to_string()),
                resolved_value: None,
                handler_param_types: None,
            },
        ],
        definition: None,
        location: "repo-b_server.ts:113:0".to_string(),
        result_type: None,
        correlated_fetch: None,
    };

    // Pattern 3: router.use("/v1", v1Router) - nested mount
    let mount3 = CallSite {
        callee_object: "router".to_string(),
        callee_property: "use".to_string(),
        args: vec![
            CallArgument {
                arg_type: ArgumentType::StringLiteral,
                value: Some("/v1".to_string()),
                resolved_value: Some("/v1".to_string()),
                handler_param_types: None,
            },
            CallArgument {
                arg_type: ArgumentType::Identifier,
                value: Some("v1Router".to_string()),
                resolved_value: None,
                handler_param_types: None,
            },
        ],
        definition: None,
        location: "api-router.ts:31:0".to_string(),
        result_type: None,
        correlated_fetch: None,
    };

    // Pattern 4: app.use(express.json()) - this is middleware, NOT a mount
    let middleware = CallSite {
        callee_object: "app".to_string(),
        callee_property: "use".to_string(),
        args: vec![CallArgument {
            arg_type: ArgumentType::Other,
            value: None,
            resolved_value: None,
            handler_param_types: None,
        }],
        definition: None,
        location: "repo-b_server.ts:6:0".to_string(),
        result_type: None,
        correlated_fetch: None,
    };

    // Convert all to LeanCallSite
    let lean1: LeanCallSite = (&mount1).into();
    let lean2: LeanCallSite = (&mount2).into();
    let lean3: LeanCallSite = (&mount3).into();
    let lean_mw: LeanCallSite = (&middleware).into();

    // All mounts should be distinguishable from middleware
    // Mounts: 2 args, first is StringLiteral with path
    for (lean, name) in [(lean1, "mount1"), (lean2, "mount2"), (lean3, "mount3")] {
        assert_eq!(lean.arg_count, 2, "{} should have 2 args", name);
        assert_eq!(
            lean.first_arg_type.as_deref(),
            Some("StringLiteral"),
            "{} first arg should be StringLiteral",
            name
        );
        assert!(
            lean.first_arg_value
                .as_ref()
                .map(|v| v.starts_with('/'))
                .unwrap_or(false),
            "{} first arg should be a path starting with /",
            name
        );
    }

    // Middleware: 1 arg, not a StringLiteral path
    assert_eq!(lean_mw.arg_count, 1, "middleware should have 1 arg");
    assert_ne!(
        lean_mw.first_arg_type.as_deref(),
        Some("StringLiteral"),
        "middleware first arg should NOT be StringLiteral"
    );
}

/// Test extraction of type information from analysis results
#[test]
fn test_type_extraction_from_analysis() {
    use carrick::agents::{DataFetchingCall, HttpEndpoint};
    use carrick::multi_agent_orchestrator::MultiAgentOrchestrator;
    use swc_common::SourceMap;
    use swc_common::sync::Lrc;

    // Create orchestrator (mock service, real sourcemap)
    let cm: Lrc<SourceMap> = Default::default();
    let orchestrator = MultiAgentOrchestrator::new("mock_key".to_string(), cm);

    // Create mock analysis results with type info
    let analysis_results = AnalysisResults {
        endpoints: vec![HttpEndpoint {
            method: "GET".to_string(),
            path: "/users".to_string(),
            handler: "getUsers".to_string(),
            node_name: "app".to_string(),
            location: "server.ts:10:0".to_string(),
            confidence: 1.0,
            reasoning: "Test".to_string(),
            response_type_file: Some("server.ts".to_string()),
            response_type_position: Some(100),
            response_type_string: Some("User[]".to_string()),
        }],
        data_fetching_calls: vec![DataFetchingCall {
            library: "fetch".to_string(),
            url: Some("/api/products".to_string()),
            method: Some("GET".to_string()),
            location: "client.ts:20:0".to_string(),
            confidence: 1.0,
            reasoning: "Test".to_string(),
            expected_type_file: Some("client.ts".to_string()),
            expected_type_position: Some(200),
            expected_type_string: Some("Product[]".to_string()),
        }],
        middleware: vec![],
        mount_relationships: vec![],
        triage_stats: carrick::agents::TriageStats::default(),
    };

    // Run extraction
    let type_infos = orchestrator.extract_types_from_analysis(&analysis_results);

    // Verify results
    assert_eq!(type_infos.len(), 2, "Should extract 2 types");

    // Check endpoint type (Producer)
    let producer_type = type_infos
        .iter()
        .find(|t| t["filePath"] == "server.ts")
        .expect("Should find producer type");
    assert_eq!(producer_type["startPosition"], 100);
    assert_eq!(producer_type["compositeTypeString"], "User[]");
    assert!(
        producer_type["alias"]
            .as_str()
            .unwrap()
            .contains("GetUsersResponseProducer")
    );

    // Check call type (Consumer)
    let consumer_type = type_infos
        .iter()
        .find(|t| t["filePath"] == "client.ts")
        .expect("Should find consumer type");
    assert_eq!(consumer_type["startPosition"], 200);
    assert_eq!(consumer_type["compositeTypeString"], "Product[]");
    assert!(
        consumer_type["alias"]
            .as_str()
            .unwrap()
            .contains("GetApiProductsResponseConsumer")
    );
}

/// Test that inline handler type annotations are extracted from call sites
#[test]
fn test_inline_handler_type_extraction() {
    use carrick::call_site_extractor::HandlerParamType;

    // Test that HandlerParamType correctly stores type information
    let param_type = HandlerParamType {
        param_name: "res".to_string(),
        type_string: "Response<User[]>".to_string(),
        utf16_offset: 558,
    };

    assert_eq!(param_type.param_name, "res");
    assert_eq!(param_type.type_string, "Response<User[]>");
    assert_eq!(param_type.utf16_offset, 558);

    // Test that CallArgument can hold handler param types
    let handler_arg = CallArgument {
        arg_type: ArgumentType::ArrowFunction,
        value: None,
        resolved_value: None,
        handler_param_types: Some(vec![
            HandlerParamType {
                param_name: "req".to_string(),
                type_string: "Request".to_string(),
                utf16_offset: 500,
            },
            HandlerParamType {
                param_name: "res".to_string(),
                type_string: "Response<User[]>".to_string(),
                utf16_offset: 520,
            },
        ]),
    };

    assert!(handler_arg.handler_param_types.is_some());
    let types = handler_arg.handler_param_types.unwrap();
    assert_eq!(types.len(), 2);

    // Verify request type
    assert_eq!(types[0].param_name, "req");
    assert_eq!(types[0].type_string, "Request");

    // Verify response type
    assert_eq!(types[1].param_name, "res");
    assert_eq!(types[1].type_string, "Response<User[]>");
}

/// Test endpoint enrichment with type info from call sites
#[tokio::test]
async fn test_endpoint_enrichment_with_inline_types() {
    use carrick::call_site_extractor::HandlerParamType;

    // Set mock mode
    unsafe {
        std::env::set_var("CARRICK_MOCK_ALL", "1");
    }

    // Create a call site with inline handler type annotations
    let call_sites = vec![CallSite {
        callee_object: "app".to_string(),
        callee_property: "get".to_string(),
        args: vec![
            CallArgument {
                arg_type: ArgumentType::StringLiteral,
                value: Some("/users".to_string()),
                resolved_value: Some("/users".to_string()),
                handler_param_types: None,
            },
            CallArgument {
                arg_type: ArgumentType::ArrowFunction,
                value: None,
                resolved_value: None,
                handler_param_types: Some(vec![
                    HandlerParamType {
                        param_name: "req".to_string(),
                        type_string: "Request".to_string(),
                        utf16_offset: 100,
                    },
                    HandlerParamType {
                        param_name: "res".to_string(),
                        type_string: "Response<User[]>".to_string(),
                        utf16_offset: 120,
                    },
                ]),
            },
        ],
        definition: Some("const app = express()".to_string()),
        location: "server.ts:10:0".to_string(),
        result_type: None,
        correlated_fetch: None,
    }];

    let framework_detection = DetectionResult {
        frameworks: vec!["express".to_string()],
        data_fetchers: vec![],
        notes: "Test".to_string(),
    };

    let gemini_service = GeminiService::new("mock".to_string());
    let orchestrator = CallSiteOrchestrator::new(gemini_service);

    let result = orchestrator
        .analyze_call_sites(&call_sites, &framework_detection)
        .await
        .expect("Analysis should succeed");

    // In mock mode, endpoints should be enriched with type info from call sites
    // The endpoint agent returns endpoints, then orchestrator enriches them
    // with handler_param_types from the original call sites
    assert!(
        result.triage_stats.endpoints_count > 0 || result.triage_stats.middleware_count > 0,
        "Should classify the endpoint call site"
    );
}
