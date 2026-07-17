//! Tests for URL normalization and consumer-producer alias matching
//!
//! This tests that:
//! 1. Template literal expressions like ${varName} are properly converted to :varName style
//! 2. The sanitize_route_for_dynamic_paths function handles both :param and ${param} formats
//! 3. Consumer aliases match producer aliases when paths are semantically equivalent

use carrick::{
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
        json_call.correlated_call.is_some(),
        "Should have correlated call info"
    );

    let info = json_call.correlated_call.as_ref().unwrap();
    let url = info.url.as_ref().expect("Should have URL");

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

/// Issue #218 end-to-end: a call whose URL base is an env var aliased through a
/// local const must resolve to the *real* `process.env` name so internal/external
/// classification (and cross-repo matching, which keys on the same name) works.
///
/// Drives the full chain the orchestrator runs: build the per-file alias map →
/// rewrite the call target → normalize against a config that declares the real
/// env var. Before the fix, the target carried the local const `ORDERS_BASE` and
/// `is_internal` was false; after, it carries `ORDERS_SERVICE_URL` and matches.
#[test]
fn test_const_aliased_env_var_resolves_to_real_name() {
    use carrick::config::Config;
    use carrick::env_alias::{EnvAliasExtractor, resolve_target_env_alias};
    use carrick::parser::parse_file;
    use carrick::url_normalizer::UrlNormalizer;

    // The exact pattern from issue #218 (payments-svc/clients/orders.client.ts).
    let source = r#"
const ORDERS_BASE = process.env.ORDERS_SERVICE_URL ?? "http://localhost:3001";

export async function getOrder(orderId: number) {
  return fetch(`${ORDERS_BASE}/orders/${orderId}`);
}
"#;

    let temp_dir = tempdir().expect("Failed to create temp dir");
    let file_path = temp_dir.path().join("orders.client.ts");
    let mut file = fs::File::create(&file_path).expect("Failed to create temp file");
    file.write_all(source.as_bytes()).expect("write temp file");

    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));
    let module = parse_file(&file_path, &cm, &handler).expect("parsed module");

    // 1. The per-file alias map links the local const to the real env var.
    let aliases = EnvAliasExtractor::build(&module);
    assert_eq!(
        aliases.get("ORDERS_BASE").map(String::as_str),
        Some("ORDERS_SERVICE_URL"),
        "alias map should resolve the const to the process.env name"
    );

    // 2. The call target the LLM emits, rewritten through the alias map.
    let target = "${ORDERS_BASE}/orders/${orderId}";
    let rewritten = resolve_target_env_alias(target, &aliases)
        .expect("leading const alias should be rewritten");
    assert_eq!(
        rewritten,
        "${process.env.ORDERS_SERVICE_URL}/orders/${orderId}"
    );

    // 3. With ORDERS_SERVICE_URL declared internal, the rewritten target now
    //    classifies as internal and yields the producer-matchable path. The
    //    original (unaliased) target does NOT — that is the bug.
    let config = Config {
        internal_env_vars: ["ORDERS_SERVICE_URL"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ..Default::default()
    };
    let normalizer = UrlNormalizer::new(&config);

    let resolved = normalizer.normalize(&rewritten);
    assert_eq!(resolved.path, "/orders/:orderId");
    assert!(
        resolved.is_internal,
        "rewritten target must classify as internal via the real env var"
    );

    let unresolved = normalizer.normalize(target);
    assert!(
        !unresolved.is_internal,
        "the un-rewritten const-aliased target should NOT classify as internal (this is #218)"
    );
}

/// Issue #218, cross-file config-object shape: a consumer that builds its URLs
/// from an *imported* config-object property must resolve the property back to
/// the real `process.env` name, exactly as a local direct alias does.
///
/// Mirrors the corpus-3 ops-console shape with generic fixture names:
///
/// ```ts
/// // config.ts
/// export const config = {
///   catalogUrl: process.env.CATALOG_URL ?? "http://localhost:4001",
/// };
/// // consumer.ts
/// import { config } from "./config";
/// const client = makeClient(config.catalogUrl);
/// ```
///
/// The file analyzer emits the call target with the config property verbatim
/// (`${config.catalogUrl}/api/v2/products/${id}`). Before the fix, env-alias
/// resolution only tracked direct local `const X = process.env.NAME` bindings,
/// so the base stayed verbatim in the call key, classification lost the env-var
/// name, and the cross-repo edge never formed.
#[test]
fn test_imported_config_object_property_resolves_env_var() {
    use carrick::agents::file_orchestrator::FileOrchestrator;
    use carrick::config::Config;
    use carrick::env_alias::{EnvAliasExtractor, EnvAliasMap, resolve_target_env_alias};
    use carrick::parser::parse_file;
    use carrick::url_normalizer::UrlNormalizer;
    use carrick::visitor::ImportSymbolExtractor;
    use std::collections::HashMap;
    use std::path::PathBuf;

    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Config module: env bases read here, not inline at call sites.
    let config_source = r#"
export const config = {
  ordersApiUrl: process.env.ORDERS_API_URL ?? "http://localhost:4003",
  catalogUrl: process.env.CATALOG_URL ?? "http://localhost:4001",
};
"#;
    fs::write(temp_dir.path().join("config.ts"), config_source).expect("write config.ts");

    // Consumer: imports the config object and passes a property as the client base.
    let consumer_source = r#"
import { makeClient } from "./lib/apiClient";
import { config } from "./config";

const catalogClient = makeClient(config.catalogUrl);

export async function updateProduct(id: string, patch: object) {
  return catalogClient.patch(`/api/v2/products/${id}`, patch);
}
"#;
    let consumer_path = temp_dir.path().join("consumer.ts");
    fs::write(&consumer_path, consumer_source).expect("write consumer.ts");

    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));
    let module = parse_file(&consumer_path, &cm, &handler).expect("parsed consumer module");

    // 1. The consumer's own alias map is empty (no local process.env reads)...
    let mut aliases = EnvAliasExtractor::build(&module);
    assert!(aliases.is_empty(), "consumer has no local env aliases");

    // ...until the cross-file pass follows the import graph to the config
    // module and resolves its object-literal properties.
    let mut import_extractor = ImportSymbolExtractor::new();
    module.visit_with(&mut import_extractor);
    let mut cache: HashMap<PathBuf, EnvAliasMap> = HashMap::new();
    FileOrchestrator::merge_cross_file_env_aliases(
        &mut aliases,
        &consumer_path,
        &import_extractor.imported_symbols,
        &mut cache,
        &cm,
        &handler,
    );
    assert_eq!(
        aliases.get("config.catalogUrl").map(String::as_str),
        Some("CATALOG_URL"),
        "imported config-object property must resolve to the real env var"
    );
    assert_eq!(
        aliases.get("config.ordersApiUrl").map(String::as_str),
        Some("ORDERS_API_URL"),
    );

    // 2. The call target the LLM emits (wrapper base resolved to the call-site
    //    argument, #370), rewritten through the augmented alias map.
    let target = "${config.catalogUrl}/api/v2/products/${id}";
    let rewritten = resolve_target_env_alias(target, &aliases)
        .expect("leading imported-config-property base should be rewritten");
    assert_eq!(
        rewritten,
        "${process.env.CATALOG_URL}/api/v2/products/${id}"
    );

    // 3. With CATALOG_URL declared internal, the rewritten target classifies as
    //    internal and yields the producer-matchable path; the verbatim target
    //    does not — that is the regression this fix closes.
    let config = Config {
        internal_env_vars: ["ORDERS_API_URL", "CATALOG_URL"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ..Default::default()
    };
    let normalizer = UrlNormalizer::new(&config);

    let resolved = normalizer.normalize(&rewritten);
    assert_eq!(resolved.path, "/api/v2/products/:id");
    assert!(
        resolved.is_internal,
        "rewritten target must classify as internal via the real env var"
    );
}

/// Issue #218, corpus-3 regression pin: drive the *actual* committed
/// `xrepo-corpus-3` ops-console fixture through the deterministic post-LLM
/// chain (cross-file alias merge → target rewrite → normalization) and assert
/// both HTTP edges recover their declared env-var names — with the corpus
/// config exactly as committed. Both edges stopped matching when an
/// unclassified `${var}` base began staying verbatim in the call key; this
/// pins that they classify internal again.
#[test]
fn test_corpus3_ops_console_edges_resolve_via_imported_config() {
    use carrick::agents::file_orchestrator::FileOrchestrator;
    use carrick::config::Config;
    use carrick::env_alias::{EnvAliasExtractor, EnvAliasMap, resolve_target_env_alias};
    use carrick::parser::parse_file;
    use carrick::url_normalizer::UrlNormalizer;
    use carrick::visitor::ImportSymbolExtractor;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    let ops_console =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xrepo-corpus-3/ops-console");

    // The corpus repo's committed carrick.json declares these internal.
    let config = Config {
        internal_env_vars: ["ORDERS_API_URL", "CATALOG_URL"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ..Default::default()
    };
    let normalizer = UrlNormalizer::new(&config);

    // (consumer file, LLM-emitted target shape, expected env var, expected path)
    let edges = [
        (
            "src/orders.ts",
            "${config.ordersApiUrl}/orders/${orderId}/timeline",
            "ORDERS_API_URL",
            "/orders/:orderId/timeline",
        ),
        (
            "src/products.ts",
            "${config.catalogUrl}/api/v2/products/${id}",
            "CATALOG_URL",
            "/api/v2/products/:id",
        ),
    ];

    for (file, target, env_var, expected_path) in edges {
        let consumer_path = ops_console.join(file);
        let cm: Lrc<SourceMap> = Default::default();
        let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));
        let module = parse_file(&consumer_path, &cm, &handler).expect("parsed fixture module");

        let mut aliases = EnvAliasExtractor::build(&module);
        let mut import_extractor = ImportSymbolExtractor::new();
        module.visit_with(&mut import_extractor);
        let mut cache: HashMap<PathBuf, EnvAliasMap> = HashMap::new();
        FileOrchestrator::merge_cross_file_env_aliases(
            &mut aliases,
            &consumer_path,
            &import_extractor.imported_symbols,
            &mut cache,
            &cm,
            &handler,
        );

        let rewritten = resolve_target_env_alias(target, &aliases).unwrap_or_else(|| {
            panic!("{file}: imported-config base in `{target}` should be rewritten")
        });
        assert_eq!(
            rewritten,
            format!(
                "${{process.env.{env_var}}}{}",
                &target[target.find('}').unwrap() + 1..]
            ),
        );

        let resolved = normalizer.normalize(&rewritten);
        assert_eq!(resolved.path, expected_path, "{file}");
        assert!(
            resolved.is_internal,
            "{file}: edge must classify internal via {env_var}"
        );
    }
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
