use carrick::{
    agents::CallSiteOrchestrator,
    call_site_extractor::{CallSiteExtractionService, CallSiteExtractor},
    framework_detector::FrameworkDetector,
    gemini_service::GeminiService,
    mount_graph::MountGraph,
    packages::Packages,
    parser::parse_file,
    visitor::{ImportSymbolExtractor, ImportedSymbol},
};
use std::collections::HashMap;
use std::path::PathBuf;
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;

async fn analyze_fixture(fixture_path: &str) -> (Vec<String>, Vec<String>) {
    unsafe {
        std::env::set_var("CARRICK_MOCK_ALL", "1");
    }

    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

    // Find files (simplified for test)
    let server_path = PathBuf::from(fixture_path).join("server.ts");
    let package_path = PathBuf::from(fixture_path).join("package.json");

    // Parse packages
    let packages = Packages::new(vec![package_path.clone()]).unwrap();

    // Parse file and extract symbols
    let mut imported_symbols = HashMap::new();
    let module = parse_file(&server_path, &cm, &handler).unwrap();
    let mut extractor = ImportSymbolExtractor::new();
    module.visit_with(&mut extractor);
    imported_symbols.extend(extractor.imported_symbols);

    // Framework Detection
    let gemini_service = GeminiService::new("mock_key".to_string());
    let detector = FrameworkDetector::new(gemini_service.clone());
    let detection = detector
        .detect_frameworks_and_libraries(&packages, &imported_symbols)
        .await
        .unwrap();

    // Call Site Extraction
    let mut extractor = CallSiteExtractor::new(server_path.clone(), cm.clone());
    module.visit_with(&mut extractor);

    let mut extraction_service = CallSiteExtractionService::new();
    extraction_service.extract_from_visitors(vec![extractor]);
    let call_sites = extraction_service.get_call_sites();

    // Orchestrator
    let orchestrator = CallSiteOrchestrator::new(gemini_service.clone());
    let analysis_results = orchestrator
        .analyze_call_sites(call_sites, &detection)
        .await
        .unwrap();

    // Mount Graph
    let imported_symbols: HashMap<String, ImportedSymbol> = HashMap::new();
    let mount_graph = MountGraph::build_from_analysis_results(&analysis_results, &imported_symbols);

    println!("DEBUG: Mounts for {}:", fixture_path);
    for mount in mount_graph.get_mounts() {
        println!(
            "  - {} mounts {} at {}",
            mount.parent, mount.child, mount.path_prefix
        );
    }

    let endpoints = mount_graph
        .get_resolved_endpoints()
        .iter()
        .map(|e| format!("{} {}", e.method, e.full_path))
        .collect::<Vec<_>>();

    let calls = mount_graph
        .get_data_calls()
        .iter()
        .map(|c| format!("{} {}", c.method, c.target_url))
        .collect::<Vec<_>>();

    (endpoints, calls)
}

#[tokio::test]
async fn test_multi_framework_equivalence() {
    // This test verifies that equivalent APIs implemented in different frameworks
    // yield identical (or highly similar) analysis results.
    // Note: In mock mode, this relies on the mock generator producing framework-specific
    // but structurally equivalent results.

    // Express (Baseline) - assumes fixture exists
    // let (express_endpoints, express_calls) = analyze_fixture("tests/fixtures/express-api").await;

    // Fastify
    let (fastify_endpoints, _) = analyze_fixture("tests/fixtures/fastify-api").await;

    // Koa
    let (koa_endpoints, _) = analyze_fixture("tests/fixtures/koa-api").await;

    // Define expected endpoints (order agnostic check)
    let expected_endpoints = vec![
        "GET /users",
        "GET /users/:id",
        "POST /orders",
        "GET /api/v1/status",
    ];

    // Verify Fastify
    for expected in &expected_endpoints {
        assert!(
            fastify_endpoints.contains(&expected.to_string()),
            "Fastify missing endpoint: {}",
            expected
        );
    }

    // Verify Koa
    for expected in &expected_endpoints {
        assert!(
            koa_endpoints.contains(&expected.to_string()),
            "Koa missing endpoint: {}",
            expected
        );
    }

    // Check call extraction (fetch calls should be identical)
    // Expect: GET http://comment-service/api/comments?userId=:id
    // Note: The exact format depends on how variable resolution works in the extractor
}
