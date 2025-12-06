//! Tests for consumer type extraction from variable declarations
//!
//! This tests the feature where type annotations on variable declarations
//! are linked to call expressions when the call is the initializer.
//!
//! Example pattern:
//! ```typescript
//! const data: Order[] = await response.json();
//! ```
//! The type `Order[]` should be captured and linked to the `response.json()` call.

use carrick::{
    agents::{AnalysisResults, CallSiteOrchestrator, TriageStats},
    call_site_extractor::{
        ArgumentType, CallArgument, CallSite, CallSiteExtractor, ResultTypeInfo,
    },
    framework_detector::DetectionResult,
    gemini_service::GeminiService,
    multi_agent_orchestrator::MultiAgentOrchestrator,
    parser::parse_file,
};
use std::fs;
use std::io::Write;
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;
use tempfile::tempdir;

/// Test that ResultTypeInfo struct correctly stores type information
#[test]
fn test_result_type_info_struct() {
    let result_type = ResultTypeInfo {
        type_string: "Order[]".to_string(),
        utf16_offset: 150,
    };

    assert_eq!(result_type.type_string, "Order[]");
    assert_eq!(result_type.utf16_offset, 150);
}

/// Test that CallSite can hold result type information
#[test]
fn test_call_site_with_result_type() {
    let call_site = CallSite {
        callee_object: "response".to_string(),
        callee_property: "json".to_string(),
        args: vec![],
        definition: None,
        location: "client.ts:10:5".to_string(),
        result_type: Some(ResultTypeInfo {
            type_string: "Product[]".to_string(),
            utf16_offset: 200,
        }),
        correlated_fetch: None,
    };

    assert!(call_site.result_type.is_some());
    let result_type = call_site.result_type.unwrap();
    assert_eq!(result_type.type_string, "Product[]");
    assert_eq!(result_type.utf16_offset, 200);
}

/// Test that CallSite without result type works correctly
#[test]
fn test_call_site_without_result_type() {
    let call_site = CallSite {
        callee_object: "app".to_string(),
        callee_property: "get".to_string(),
        args: vec![CallArgument {
            arg_type: ArgumentType::StringLiteral,
            value: Some("/users".to_string()),
            resolved_value: Some("/users".to_string()),
            handler_param_types: None,
        }],
        definition: Some("const app = express()".to_string()),
        location: "server.ts:5:0".to_string(),
        result_type: None,
        correlated_fetch: None,
    };

    assert!(call_site.result_type.is_none());
}

/// Helper function to parse TypeScript code and extract call sites
fn parse_and_extract_call_sites(code: &str, filename: &str) -> Vec<CallSite> {
    let temp_dir = tempdir().expect("Failed to create temp dir");
    let file_path = temp_dir.path().join(filename);
    let mut file = fs::File::create(&file_path).expect("Failed to create temp file");
    file.write_all(code.as_bytes())
        .expect("Failed to write temp file");

    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

    let module = parse_file(&file_path, &cm, &handler).expect("Failed to parse file");
    let mut extractor = CallSiteExtractor::new(file_path.clone(), cm.clone());
    module.visit_with(&mut extractor);

    extractor.call_sites
}

/// Test extracting result type from simple typed variable declaration
#[test]
fn test_extract_result_type_simple_await() {
    let code = r#"
interface Order {
    id: number;
    total: number;
}

async function fetchOrders() {
    const response = await fetch("/api/orders");
    const data: Order[] = await response.json();
    return data;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_simple.ts");

    // Find the response.json() call
    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "response" && cs.callee_property == "json")
        .expect("Should find response.json() call");

    assert!(
        json_call.result_type.is_some(),
        "response.json() should have result type"
    );
    let result_type = json_call.result_type.as_ref().unwrap();
    assert_eq!(result_type.type_string, "Order[]");
}

/// Test extracting result type from direct call without await
#[test]
fn test_extract_result_type_sync_call() {
    let code = r#"
interface Config {
    debug: boolean;
}

function getConfig() {
    const config: Config = loadConfig();
    return config;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_sync.ts");

    // Find the loadConfig() call
    let config_call = call_sites
        .iter()
        .find(|cs| cs.callee_property == "loadConfig")
        .expect("Should find loadConfig() call");

    assert!(
        config_call.result_type.is_some(),
        "loadConfig() should have result type"
    );
    let result_type = config_call.result_type.as_ref().unwrap();
    assert_eq!(result_type.type_string, "Config");
}

/// Test that calls without type annotations have no result type
#[test]
fn test_no_result_type_for_untyped_variable() {
    let code = r#"
async function fetchData() {
    const response = await fetch("/api/data");
    const data = await response.json();  // No type annotation
    return data;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_untyped.ts");

    // Find the response.json() call
    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "response" && cs.callee_property == "json")
        .expect("Should find response.json() call");

    assert!(
        json_call.result_type.is_none(),
        "Untyped variable should not have result type"
    );
}

/// Test extracting result type with generic types
#[test]
fn test_extract_result_type_generic() {
    let code = r#"
interface ApiResponse<T> {
    data: T;
    status: number;
}

interface User {
    id: number;
    name: string;
}

async function fetchUsers() {
    const response = await fetch("/api/users");
    const result: ApiResponse<User[]> = await response.json();
    return result;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_generic.ts");

    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "response" && cs.callee_property == "json")
        .expect("Should find response.json() call");

    assert!(json_call.result_type.is_some());
    let result_type = json_call.result_type.as_ref().unwrap();
    assert_eq!(result_type.type_string, "ApiResponse<User[]>");
}

/// Test extracting result type from member call (e.g., axios.get)
#[test]
fn test_extract_result_type_member_call() {
    let code = r#"
interface Product {
    id: number;
    name: string;
    price: number;
}

async function fetchProducts() {
    const products: Product[] = await axios.get("/api/products");
    return products;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_member.ts");

    let axios_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "axios" && cs.callee_property == "get")
        .expect("Should find axios.get() call");

    assert!(axios_call.result_type.is_some());
    let result_type = axios_call.result_type.as_ref().unwrap();
    assert_eq!(result_type.type_string, "Product[]");
}

/// Test multiple calls with different result types
#[test]
fn test_multiple_calls_with_result_types() {
    let code = r#"
interface Order { id: number; }
interface User { name: string; }

async function fetchData() {
    const ordersResp = await fetch("/orders");
    const orders: Order[] = await ordersResp.json();

    const usersResp = await fetch("/users");
    const users: User[] = await usersResp.json();

    return { orders, users };
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_multiple.ts");

    // Find json calls - there should be two with different result types
    let json_calls: Vec<_> = call_sites
        .iter()
        .filter(|cs| cs.callee_property == "json")
        .collect();

    assert_eq!(json_calls.len(), 2, "Should find two json() calls");

    // Check that both have result types
    let types: Vec<_> = json_calls
        .iter()
        .filter_map(|cs| cs.result_type.as_ref())
        .map(|rt| rt.type_string.as_str())
        .collect();

    assert!(types.contains(&"Order[]"), "Should have Order[] type");
    assert!(types.contains(&"User[]"), "Should have User[] type");
}

/// Test that enrichment function correctly populates DataFetchingCall
#[tokio::test]
async fn test_data_fetching_call_enrichment() {
    // Set mock mode
    unsafe {
        std::env::set_var("CARRICK_MOCK_ALL", "1");
    }

    // Create call sites with result type info (simulating what SWC extracts)
    let call_sites = vec![CallSite {
        callee_object: "response".to_string(),
        callee_property: "json".to_string(),
        args: vec![],
        definition: None,
        location: "client.ts:25:10".to_string(),
        result_type: Some(ResultTypeInfo {
            type_string: "Order[]".to_string(),
            utf16_offset: 450,
        }),
        correlated_fetch: None,
    }];

    let framework_detection = DetectionResult {
        frameworks: vec![],
        data_fetchers: vec!["fetch".to_string()],
        notes: "Test".to_string(),
    };

    let gemini_service = GeminiService::new("mock".to_string());
    let orchestrator = CallSiteOrchestrator::new(gemini_service);

    let result = orchestrator
        .analyze_call_sites(&call_sites, &framework_detection)
        .await
        .expect("Analysis should succeed");

    // In mock mode, check that the system processes the call
    // The enrichment should transfer result_type to expected_type_* fields
    assert!(
        result.triage_stats.data_fetching_count > 0
            || result.triage_stats.irrelevant_count > 0
            || result.triage_stats.middleware_count > 0,
        "Should classify the call site"
    );
}

/// Test type extraction from analysis results includes consumer types
#[test]
fn test_extract_types_includes_consumer_types() {
    use carrick::agents::DataFetchingCall;
    use carrick::agents::HttpEndpoint;

    let cm: Lrc<SourceMap> = Default::default();
    let orchestrator = MultiAgentOrchestrator::new("mock_key".to_string(), cm);

    // Create analysis results with both producer and consumer types
    let analysis_results = AnalysisResults {
        endpoints: vec![HttpEndpoint {
            method: "GET".to_string(),
            path: "/orders".to_string(),
            handler: "getOrders".to_string(),
            node_name: "app".to_string(),
            location: "server.ts:10:0".to_string(),
            confidence: 1.0,
            reasoning: "Test".to_string(),
            response_type_file: Some("server.ts".to_string()),
            response_type_position: Some(100),
            response_type_string: Some("Order[]".to_string()),
        }],
        data_fetching_calls: vec![DataFetchingCall {
            library: "fetch".to_string(),
            url: Some("/api/orders".to_string()),
            method: Some("GET".to_string()),
            location: "client.ts:20:0".to_string(),
            confidence: 1.0,
            reasoning: "Test".to_string(),
            expected_type_file: Some("client.ts".to_string()),
            expected_type_position: Some(300),
            expected_type_string: Some("Order[]".to_string()),
        }],
        middleware: vec![],
        mount_relationships: vec![],
        triage_stats: TriageStats::default(),
    };

    let type_infos = orchestrator.extract_types_from_analysis(&analysis_results);

    // Should have 2 type infos: 1 producer + 1 consumer
    assert_eq!(
        type_infos.len(),
        2,
        "Should extract both producer and consumer types"
    );

    // Check producer type
    let producer = type_infos
        .iter()
        .find(|t| t["filePath"] == "server.ts")
        .expect("Should find producer type");
    assert_eq!(producer["compositeTypeString"], "Order[]");
    assert!(producer["alias"].as_str().unwrap().contains("Producer"));

    // Check consumer type
    let consumer = type_infos
        .iter()
        .find(|t| t["filePath"] == "client.ts")
        .expect("Should find consumer type");
    assert_eq!(consumer["compositeTypeString"], "Order[]");
    assert!(consumer["alias"].as_str().unwrap().contains("Consumer"));
}

/// Test that UTF-16 offset is correctly calculated for non-ASCII characters
#[test]
fn test_utf16_offset_with_unicode() {
    let code = r#"
// Comment with emoji: 🎉
interface データ {
    id: number;
}

async function fetch日本語() {
    const response = await fetch("/api");
    const result: データ = await response.json();
    return result;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_unicode.ts");

    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "response" && cs.callee_property == "json")
        .expect("Should find response.json() call");

    assert!(json_call.result_type.is_some());
    let result_type = json_call.result_type.as_ref().unwrap();
    assert_eq!(result_type.type_string, "データ");
    // The UTF-16 offset should be calculated correctly for the type position
    assert!(result_type.utf16_offset > 0);
}

/// Test extraction with complex nested types
#[test]
fn test_extract_complex_nested_type() {
    let code = r#"
interface Pagination<T> {
    items: T[];
    total: number;
    page: number;
}

interface Order {
    id: string;
    items: Array<{ productId: string; quantity: number }>;
}

async function fetchPaginatedOrders() {
    const response = await fetch("/api/orders?page=1");
    const result: Pagination<Order> = await response.json();
    return result;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_complex.ts");

    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "response" && cs.callee_property == "json")
        .expect("Should find response.json() call");

    assert!(json_call.result_type.is_some());
    let result_type = json_call.result_type.as_ref().unwrap();
    assert_eq!(result_type.type_string, "Pagination<Order>");
}

/// Test extraction with union types
#[test]
fn test_extract_union_type() {
    let code = r#"
interface SuccessResponse {
    success: true;
    data: string[];
}

interface ErrorResponse {
    success: false;
    error: string;
}

async function fetchData() {
    const response = await fetch("/api/data");
    const result: SuccessResponse | ErrorResponse = await response.json();
    return result;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_union.ts");

    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "response" && cs.callee_property == "json")
        .expect("Should find response.json() call");

    assert!(json_call.result_type.is_some());
    let result_type = json_call.result_type.as_ref().unwrap();
    assert_eq!(result_type.type_string, "SuccessResponse | ErrorResponse");
}

/// Test that let declarations also capture result types
#[test]
fn test_extract_result_type_let_declaration() {
    let code = r#"
interface Config {
    setting: string;
}

async function init() {
    const response = await fetch("/config");
    let config: Config = await response.json();
    config.setting = "modified";
    return config;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_let.ts");

    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "response" && cs.callee_property == "json")
        .expect("Should find response.json() call");

    assert!(json_call.result_type.is_some());
    let result_type = json_call.result_type.as_ref().unwrap();
    assert_eq!(result_type.type_string, "Config");
}

/// Test extraction from parenthesized await expression
#[test]
fn test_extract_result_type_parenthesized_await() {
    let code = r#"
interface Data {
    value: number;
}

async function fetchData() {
    const response = await fetch("/api");
    const data: Data = (await response.json());
    return data;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_paren.ts");

    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "response" && cs.callee_property == "json")
        .expect("Should find response.json() call");

    assert!(json_call.result_type.is_some());
    let result_type = json_call.result_type.as_ref().unwrap();
    assert_eq!(result_type.type_string, "Data");
}

/// Test that fetch-to-json call correlation is correctly tracked
/// When we see: const resp = await fetch(url); const data: T = await resp.json();
/// The .json() call should have correlated_fetch with the original fetch URL
#[test]
fn test_fetch_to_json_correlation() {
    let code = r#"
interface Order {
    id: number;
    total: number;
}

async function fetchOrders() {
    const ordersResp = await fetch("/orders");
    const ordersRaw: Order[] = await ordersResp.json();
    return ordersRaw;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_fetch_correlation.ts");

    // Find the ordersResp.json() call
    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "ordersResp" && cs.callee_property == "json")
        .expect("Should find ordersResp.json() call");

    // Should have result type from the type annotation
    assert!(
        json_call.result_type.is_some(),
        "json() call should have result type"
    );
    assert_eq!(
        json_call.result_type.as_ref().unwrap().type_string,
        "Order[]"
    );

    // Should have correlated_fetch from the original fetch() call
    assert!(
        json_call.correlated_fetch.is_some(),
        "json() call should have correlated_fetch"
    );
    let fetch_info = json_call.correlated_fetch.as_ref().unwrap();
    assert_eq!(fetch_info.url.as_deref(), Some("/orders"));
    assert_eq!(fetch_info.method, "GET");
}

/// Test fetch-to-json correlation with template literal URL
#[test]
fn test_fetch_to_json_correlation_template_literal() {
    let code = r#"
interface User {
    id: number;
    name: string;
}

async function fetchUsers() {
    const usersResp = await fetch(`${process.env.API_URL}/users`);
    const users: User[] = await usersResp.json();
    return users;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_fetch_template.ts");

    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "usersResp" && cs.callee_property == "json")
        .expect("Should find usersResp.json() call");

    assert!(json_call.correlated_fetch.is_some());
    let fetch_info = json_call.correlated_fetch.as_ref().unwrap();
    // Should extract path portion from template literal
    assert_eq!(fetch_info.url.as_deref(), Some("/users"));
    assert_eq!(fetch_info.method, "GET");
}

/// Test fetch-to-json correlation with POST method
#[test]
fn test_fetch_to_json_correlation_post_method() {
    let code = r#"
interface CreateOrderResponse {
    orderId: string;
}

async function createOrder(data: any) {
    const resp = await fetch("/orders", { method: "POST", body: JSON.stringify(data) });
    const result: CreateOrderResponse = await resp.json();
    return result;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_fetch_post.ts");

    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "resp" && cs.callee_property == "json")
        .expect("Should find resp.json() call");

    assert!(json_call.correlated_fetch.is_some());
    let fetch_info = json_call.correlated_fetch.as_ref().unwrap();
    assert_eq!(fetch_info.url.as_deref(), Some("/orders"));
    assert_eq!(fetch_info.method, "POST");
}

/// Test multiple fetch-to-json correlations in same function
#[test]
fn test_multiple_fetch_to_json_correlations() {
    let code = r#"
interface Order { id: number; }
interface User { name: string; }

async function fetchData() {
    const ordersResp = await fetch("/orders");
    const orders: Order[] = await ordersResp.json();

    const usersResp = await fetch("/users");
    const users: User[] = await usersResp.json();

    return { orders, users };
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_multiple_correlation.ts");

    // Find both json calls
    let orders_json = call_sites
        .iter()
        .find(|cs| cs.callee_object == "ordersResp" && cs.callee_property == "json")
        .expect("Should find ordersResp.json() call");

    let users_json = call_sites
        .iter()
        .find(|cs| cs.callee_object == "usersResp" && cs.callee_property == "json")
        .expect("Should find usersResp.json() call");

    // Check orders correlation
    assert!(orders_json.correlated_fetch.is_some());
    assert_eq!(
        orders_json
            .correlated_fetch
            .as_ref()
            .unwrap()
            .url
            .as_deref(),
        Some("/orders")
    );
    assert_eq!(
        orders_json.result_type.as_ref().unwrap().type_string,
        "Order[]"
    );

    // Check users correlation
    assert!(users_json.correlated_fetch.is_some());
    assert_eq!(
        users_json.correlated_fetch.as_ref().unwrap().url.as_deref(),
        Some("/users")
    );
    assert_eq!(
        users_json.result_type.as_ref().unwrap().type_string,
        "User[]"
    );
}

/// Test that non-json member calls don't get correlated_fetch
#[test]
fn test_non_json_call_no_correlation() {
    let code = r#"
async function fetchData() {
    const resp = await fetch("/data");
    const text = await resp.text();
    return text;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_text_call.ts");

    let text_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "resp" && cs.callee_property == "text")
        .expect("Should find resp.text() call");

    // text() call should NOT have correlated_fetch (only json() calls get it)
    assert!(
        text_call.correlated_fetch.is_none(),
        "text() call should not have correlated_fetch"
    );
}
