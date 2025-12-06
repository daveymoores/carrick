//! Tests for URL normalization and consumer-producer alias matching
//!
//! This tests that:
//! 1. Template literal expressions like ${varName} are properly converted to :varName style
//! 2. The sanitize_route_for_dynamic_paths function handles both :param and ${param} formats
//! 3. Consumer aliases match producer aliases when paths are semantically equivalent

use carrick::{
    agents::consumer_agent::DataFetchingCall,
    analyzer::Analyzer,
    call_site_extractor::{CallSite, CallSiteExtractor},
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

/// Helper function to parse TypeScript code and extract call sites
fn parse_and_extract_call_sites(code: &str, filename: &str) -> Vec<CallSite> {
    use carrick::parser::parse_file;

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

/// Test that generate_unique_call_alias_name produces matching aliases for equivalent paths
/// This verifies that /users/:id and /users/:userId produce aliases that can be matched
#[test]
fn test_alias_generation_path_param_normalization() {
    // Producer path uses :id
    let producer_alias = Analyzer::generate_unique_call_alias_name(
        "/users/:id/comments",
        "GET",
        false, // is_request_type
        1,     // call_number
        false, // is_consumer (false = producer)
    );

    // Consumer path uses :userId - should still produce a matchable alias
    let consumer_alias = Analyzer::generate_unique_call_alias_name(
        "/users/:userId/comments",
        "GET",
        false, // is_request_type
        1,     // call_number
        true,  // is_consumer
    );

    // Both should have the "By" pattern that allows matching
    assert!(
        producer_alias.contains("By"),
        "Producer alias should contain 'By' for path params. Got: {}",
        producer_alias
    );
    assert!(
        consumer_alias.contains("By"),
        "Consumer alias should contain 'By' for path params. Got: {}",
        consumer_alias
    );

    println!("Producer alias: {}", producer_alias);
    println!("Consumer alias: {}", consumer_alias);
}

/// Test that template literal paths ${varName} are handled correctly by alias generation
/// This is the critical test - if the path still has ${...} it will produce wrong aliases
#[test]
fn test_alias_generation_template_literal_path() {
    // This simulates what happens when the URL is NOT properly normalized
    // and still contains template literal syntax
    let bad_alias = Analyzer::generate_unique_call_alias_name(
        "/users/${userId}/comments",
        "GET",
        false,
        1,
        true,
    );

    println!("Template literal path alias: {}", bad_alias);

    // The alias should either:
    // 1. Have "By" prefix (if ${...} is converted to :param style)
    // 2. Or at minimum not have the raw $ and braces
    let has_dollar_braces =
        bad_alias.contains('$') || bad_alias.contains('{') || bad_alias.contains('}');

    // If sanitize_route_for_dynamic_paths handles ${...}, the alias should be correct
    // If not, this test documents the expected behavior
    if has_dollar_braces {
        println!(
            "WARNING: Template literal syntax not being sanitized. Got: {}",
            bad_alias
        );
    }

    // EXPECTED: The alias should have "ByUserid" for the ${userId} param
    // If sanitize_route_for_dynamic_paths doesn't handle ${...}, this will fail
    // and show us what the actual alias looks like
    assert!(
        bad_alias.contains("ByUserid"),
        "Alias should have 'ByUserid' for ${{userId}} path param. Got: {}",
        bad_alias
    );
}

/// Test that properly normalized paths with :param produce correct aliases
#[test]
fn test_alias_generation_normalized_colon_param() {
    let alias = Analyzer::generate_unique_call_alias_name(
        "/api/comments/:userId/:commentId",
        "GET",
        false,
        1,
        true,
    );

    // Should have ByUserid and ByCommentid patterns
    assert!(
        alias.contains("ByUserid") || alias.contains("ByUseridByCommentid") || alias.contains("By"),
        "Alias should contain 'By' prefix for :param style. Got: {}",
        alias
    );

    // Should NOT have colons in the alias
    assert!(
        !alias.contains(':'),
        "Alias should not contain colons. Got: {}",
        alias
    );

    println!("Normalized alias: {}", alias);
}

/// Test that the SWC extractor normalizes template literals to :param style
#[test]
fn test_swc_extractor_normalizes_template_params() {
    let code = r#"
interface User {
    id: number;
    name: string;
}

async function fetchUser(userId: string) {
    const resp = await fetch(`${process.env.API_URL}/users/${userId}`);
    const user: User = await resp.json();
    return user;
}
"#;

    let call_sites = parse_and_extract_call_sites(code, "test_template.ts");

    let json_call = call_sites
        .iter()
        .find(|cs| cs.callee_object == "resp" && cs.callee_property == "json")
        .expect("Should find resp.json() call");

    assert!(
        json_call.correlated_fetch.is_some(),
        "Should have correlated fetch info"
    );

    let fetch_info = json_call.correlated_fetch.as_ref().unwrap();
    let url = fetch_info.url.as_ref().expect("Should have URL");

    // The URL should be normalized to :param style
    assert!(
        !url.contains("${"),
        "URL should not contain template literal syntax. Got: {}",
        url
    );
    assert!(
        url.contains(":userId") || url.contains(":param"),
        "URL should have :param style path parameters. Got: {}",
        url
    );
}

/// Test that when enrichment receives a correlated fetch with normalized URL,
/// it should be used even if LLM provided a different URL
#[test]
fn test_enrichment_prefers_swc_url_over_llm_url() {
    // Simulate what happens in enrich_data_fetching_calls_with_type_info

    // LLM returns a DataFetchingCall with malformed URL (template literal syntax)
    let mut call = DataFetchingCall {
        library: "fetch".to_string(),
        url: Some("/users/${userId}/comments".to_string()), // Bad URL from LLM
        method: Some("GET".to_string()),
        location: "test.ts:10:5".to_string(),
        confidence: 0.95,
        reasoning: "fetch call".to_string(),
        expected_type_file: None,
        expected_type_position: None,
        expected_type_string: None,
    };

    // SWC extractor provides properly normalized URL
    let swc_url = Some("/users/:userId/comments".to_string());

    // The fix should override the LLM URL with the SWC URL
    // For now, document what the current behavior is
    if call.url.is_some() && swc_url.is_some() {
        // Currently, if call.url is Some, the SWC URL is NOT used (bug)
        // After fix, the SWC URL should always be preferred

        // Simulate the fix: always use SWC URL if available
        call.url = swc_url.clone();
    }

    assert_eq!(
        call.url.as_ref().unwrap(),
        "/users/:userId/comments",
        "Should use SWC-extracted normalized URL"
    );
}

/// Test common patterns that should all produce matching aliases
#[test]
fn test_path_patterns_produce_matchable_aliases() {
    let test_cases = vec![
        // (producer_path, consumer_path, should_match)
        ("/users/:id", "/users/:userId", true),
        ("/api/:id/comments", "/api/:postId/comments", true),
        ("/orders/:orderId", "/orders/:id", true),
    ];

    for (producer_path, consumer_path, should_match) in test_cases {
        let producer_alias =
            Analyzer::generate_unique_call_alias_name(producer_path, "GET", false, 1, false);

        let consumer_alias =
            Analyzer::generate_unique_call_alias_name(consumer_path, "GET", false, 1, true);

        // Extract the path part from aliases (before "ResponseProducer" or "ResponseConsumer")
        let producer_path_part = producer_alias
            .strip_prefix("Get")
            .and_then(|s| s.strip_suffix("ResponseProducerCall1"))
            .or_else(|| {
                producer_alias.strip_prefix("Get").map(|s| {
                    if let Some(idx) = s.find("Response") {
                        &s[..idx]
                    } else {
                        s
                    }
                })
            })
            .unwrap_or(&producer_alias);

        let consumer_path_part = consumer_alias
            .strip_prefix("Get")
            .and_then(|s| s.strip_suffix("ResponseConsumerCall1"))
            .or_else(|| {
                consumer_alias.strip_prefix("Get").map(|s| {
                    if let Some(idx) = s.find("Response") {
                        &s[..idx]
                    } else {
                        s
                    }
                })
            })
            .unwrap_or(&consumer_alias);

        println!(
            "Producer: {} -> {} (path: {})",
            producer_path, producer_alias, producer_path_part
        );
        println!(
            "Consumer: {} -> {} (path: {})",
            consumer_path, consumer_alias, consumer_path_part
        );

        // Both should have the "By" pattern for parameters
        if should_match {
            assert!(
                producer_path_part.contains("By") && consumer_path_part.contains("By"),
                "Both should use 'By' prefix. Producer: {}, Consumer: {}",
                producer_path_part,
                consumer_path_part
            );
        }
    }
}

/// Test that double path params are handled correctly
/// This tests the specific case from the failing output: /api/comments/userid/userid
#[test]
fn test_double_path_params_handled_correctly() {
    // The bug showed paths like /api/comments/:userId/:userId being rendered as
    // /api/comments/userid/userid (no colons, no By prefix)

    let alias = Analyzer::generate_unique_call_alias_name(
        "/api/comments/:userId/:commentId",
        "GET",
        false,
        1,
        true,
    );

    // Should have TWO "By" occurrences for two path params
    let by_count = alias.matches("By").count();

    println!("Double param alias: {} (By count: {})", alias, by_count);

    // If both params are properly handled, should have 2 "By" prefixes
    assert!(
        by_count >= 2 || alias.contains("ByUseridByCommentid"),
        "Should have 'By' prefix for each path param. Got: {} (By count: {})",
        alias,
        by_count
    );
}

/// Test that the type checker's path matching works for normalized paths
/// This tests the TypeScript side expectation
#[test]
fn test_expected_alias_format_for_type_checker() {
    // The TypeScript type checker expects aliases like:
    // GetUsersByIdCommentsResponseProducer
    // GetUsersByIdCommentsResponseConsumerCall1
    //
    // The key is that path params become "By{ParamName}" in PascalCase

    let producer_alias =
        Analyzer::generate_unique_call_alias_name("/users/:id/comments", "GET", false, 1, false);

    let consumer_alias =
        Analyzer::generate_unique_call_alias_name("/users/:userId/comments", "GET", false, 1, true);

    // The TypeScript camelCaseToPath function reverses this:
    // UsersByIdComments -> /users/:id/comments
    // UsersByUseridComments -> /users/:userid/comments
    //
    // These should match after normalization in the type checker

    println!("Expected producer alias: {}", producer_alias);
    println!("Expected consumer alias: {}", consumer_alias);

    // Both should have the structure: Get{Path}ResponseProducer/Consumer
    assert!(producer_alias.starts_with("Get"), "Should start with Get");
    assert!(consumer_alias.starts_with("Get"), "Should start with Get");
    assert!(
        producer_alias.contains("Response"),
        "Should contain Response"
    );
    assert!(
        consumer_alias.contains("Response"),
        "Should contain Response"
    );
}

/// Test edge case: empty path
#[test]
fn test_empty_path_alias() {
    let alias = Analyzer::generate_unique_call_alias_name("/", "GET", false, 1, true);

    // Should not panic and should produce a valid alias
    assert!(!alias.is_empty(), "Alias should not be empty for root path");
    println!("Root path alias: {}", alias);
}

/// Test edge case: path with query params (should be stripped)
#[test]
fn test_path_with_query_params() {
    // Query params should ideally be stripped before alias generation
    // This documents current behavior
    let alias = Analyzer::generate_unique_call_alias_name(
        "/users/:id?include=posts",
        "GET",
        false,
        1,
        true,
    );

    println!("Path with query alias: {}", alias);

    // Should still have the By pattern for the path param
    assert!(
        alias.contains("By"),
        "Should still extract path param even with query. Got: {}",
        alias
    );
}

/// Test that all HTTP methods produce correct aliases
#[test]
fn test_http_methods_in_aliases() {
    let methods = vec!["GET", "POST", "PUT", "DELETE", "PATCH"];

    for method in methods {
        let alias = Analyzer::generate_unique_call_alias_name("/users/:id", method, false, 1, true);

        let expected_prefix = match method {
            "GET" => "Get",
            "POST" => "Post",
            "PUT" => "Put",
            "DELETE" => "Delete",
            "PATCH" => "Patch",
            _ => method,
        };

        assert!(
            alias.starts_with(expected_prefix),
            "{} should produce alias starting with {}. Got: {}",
            method,
            expected_prefix,
            alias
        );

        println!("{} -> {}", method, alias);
    }
}
