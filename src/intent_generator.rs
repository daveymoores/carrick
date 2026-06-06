//! Function intent generator.
//!
//! Generates short natural-language descriptions of what each function
//! intends to do, using a small LLM model. Functions are processed in
//! dependency order (leaves first) so that when a function calls other
//! local functions, those functions' intents are included in the prompt
//! for richer compositional understanding.
//!
//! After intent generation, `body_source` is stripped from all function
//! definitions so that source code is not uploaded to AWS. The intent
//! serves as the index; GitHub is the source of truth for code.

use crate::agent_service::AgentService;
use crate::visitor::{FunctionCallRef, FunctionDefinition, ImportedSymbol};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use tracing::{debug, warn};

/// Bump when the `/generate-intent` model or prompt template changes so that
/// intents cached by content hash are regenerated rather than reused. The model
/// and prompt live in the lambda (carrick-cloud), invisible to this crate, so
/// this constant is the manual invalidation lever.
const INTENT_CACHE_VERSION: u32 = 1;

/// Content hash of the exact inputs that determine a function's generated
/// intent: the cache version, the function body, and its callees' intents.
/// Callee intents are sorted so set-equal contexts hash identically regardless
/// of discovery order. Fields are length-delimited so concatenation is
/// unambiguous.
fn compute_intent_hash(body: &str, called_intents: &[String]) -> String {
    let mut sorted: Vec<&String> = called_intents.iter().collect();
    sorted.sort();

    let mut hasher = Sha256::new();
    hasher.update(INTENT_CACHE_VERSION.to_le_bytes());
    hasher.update((body.len() as u64).to_le_bytes());
    hasher.update(body.as_bytes());
    hasher.update((sorted.len() as u64).to_le_bytes());
    for ci in sorted {
        hasher.update((ci.len() as u64).to_le_bytes());
        hasher.update(ci.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Build a `content_hash -> intent` map from a previous scan's function
/// definitions, keeping only entries that carry both an intent and the hash of
/// the inputs that produced it. Passed into [`generate_function_intents`] so an
/// unchanged function (same body + same callee intents) reuses its prior intent
/// without another `/generate-intent` call. Definitions from a scan that
/// predates content hashing simply lack `intent_input_hash` and are skipped
/// (treated as cache misses).
pub fn intents_by_hash(
    function_definitions: &HashMap<String, FunctionDefinition>,
) -> HashMap<String, String> {
    function_definitions
        .values()
        .filter_map(|def| match (&def.intent_input_hash, &def.intent) {
            (Some(hash), Some(intent)) => Some((hash.clone(), intent.clone())),
            _ => None,
        })
        .collect()
}

/// Generate intents for all exported functions that have body source.
///
/// After generation:
/// - Each function's `intent` is populated with a 1-2 sentence description
/// - Each function's `intent_input_hash` records the content hash that produced it
/// - Each function's `calls` is populated with references to local callees
/// - `body_source` is stripped from ALL functions (source stays in GitHub, not AWS)
///
/// `prev_intents_by_hash` is a `content_hash -> intent` map from the previous
/// scan (see [`intents_by_hash`]). A function whose freshly-computed hash is
/// present in the map reuses that intent without calling `/generate-intent`.
/// Pass an empty map for a full (non-incremental) scan.
pub async fn generate_function_intents(
    agent_service: &AgentService,
    function_definitions: &mut HashMap<String, FunctionDefinition>,
    _imported_symbols: &HashMap<String, ImportedSymbol>,
    prev_intents_by_hash: &HashMap<String, String>,
) {
    // Process all named functions with body source
    let eligible: Vec<String> = function_definitions
        .iter()
        .filter(|(_, def)| def.body_source.is_some())
        .map(|(name, _)| name.clone())
        .collect();

    if eligible.is_empty() {
        strip_body_source(function_definitions);
        return;
    }

    debug!("Generating intents for {} function(s)", eligible.len());

    // Build a local call graph: for each function, which other local functions does it reference?
    let local_fn_names: HashSet<&str> = function_definitions.keys().map(|s| s.as_str()).collect();
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();

    for name in &eligible {
        if let Some(def) = function_definitions.get(name)
            && let Some(ref body) = def.body_source
        {
            let called: Vec<String> = local_fn_names
                .iter()
                .filter(|&&fn_name| fn_name != name.as_str() && body.contains(fn_name))
                .map(|&s| s.to_string())
                .collect();
            deps.insert(name.clone(), called);
        }
    }

    // Populate the `calls` field on each function with references to callees
    for name in &eligible {
        if let Some(called) = deps.get(name) {
            let call_refs: Vec<FunctionCallRef> = called
                .iter()
                .filter_map(|callee_name| {
                    function_definitions
                        .get(callee_name)
                        .map(|callee_def| FunctionCallRef {
                            name: callee_name.clone(),
                            file_path: callee_def.file_path.to_string_lossy().to_string(),
                            line_number: callee_def.line_number,
                        })
                })
                .collect();
            if let Some(def) = function_definitions.get_mut(name) {
                def.calls = call_refs;
            }
        }
    }

    // Topological sort into levels: functions at the same level can run in parallel
    let levels = topological_levels(&eligible, &deps);

    // Generate intents level by level — within each level, calls run in parallel.
    // Both the system instruction and user-prompt template live in the
    // /generate-intent lambda (carrick-cloud/lambdas/generate-intent/index.js).
    //
    // Caching is content-addressed: for each function we compute a hash over its
    // body and its callees' (already-resolved) intents. If that hash was seen in
    // the previous scan, we reuse the prior intent without a lambda call. This
    // both avoids redundant calls for unchanged code AND correctly invalidates a
    // caller when a callee's intent changed (its `called_intents` differ, so its
    // hash differs). Processing leaves-first guarantees callee intents are
    // resolved before their callers are hashed.
    //
    // `intents` holds the resolved intent per function (reused or freshly
    // generated); `hashes` holds the content hash that produced each one, to be
    // persisted on the definition for the next scan.
    let mut intents: HashMap<String, String> = HashMap::new();
    let mut hashes: HashMap<String, String> = HashMap::new();
    let mut reused = 0usize;
    let mut generated = 0usize;

    for level in &levels {
        // Compute each function's called_intents context and content hash, then
        // split into cache hits (reuse) and misses (call the lambda).
        struct Pending {
            name: String,
            body: String,
            called_intents: Vec<String>,
            hash: String,
        }
        let mut to_generate: Vec<Pending> = Vec::new();

        for name in level {
            let Some(def) = function_definitions.get(name) else {
                continue;
            };
            let Some(body) = def.body_source.as_ref() else {
                continue;
            };

            let called_intents: Vec<String> = deps
                .get(name)
                .map(|called| {
                    called
                        .iter()
                        .filter_map(|callee| {
                            intents
                                .get(callee)
                                .map(|intent| format!("- {}: {}", callee, intent))
                        })
                        .collect()
                })
                .unwrap_or_default();

            let hash = compute_intent_hash(body, &called_intents);

            if let Some(prev_intent) = prev_intents_by_hash.get(&hash) {
                // Identical body + callee context as a prior scan — reuse.
                intents.insert(name.clone(), prev_intent.clone());
                hashes.insert(name.clone(), hash);
                reused += 1;
            } else {
                to_generate.push(Pending {
                    name: name.clone(),
                    body: body.clone(),
                    called_intents,
                    hash,
                });
            }
        }

        // Run all cache-miss lambda calls for this level in parallel.
        let futures: Vec<_> = to_generate
            .iter()
            .map(|pending| async move {
                let payload = serde_json::json!({
                    "name": pending.name,
                    "body": pending.body,
                    "called_intents": pending.called_intents,
                });
                let result = agent_service
                    .post_to_lambda("/generate-intent", &payload, &pending.name)
                    .await;
                (pending.name.clone(), pending.hash.clone(), result)
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        for (name, hash, result) in results {
            match result {
                Ok(intent) => {
                    let intent = intent.trim().to_string();
                    if !intent.is_empty() && intent.len() < 500 {
                        hashes.insert(name.clone(), hash);
                        intents.insert(name, intent);
                        generated += 1;
                    } else {
                        // Empty or over-long response: drop it. The function
                        // keeps `intent = None`, so it (and its callers) retry
                        // next scan. Log it — otherwise this is a silent,
                        // permanent cache miss.
                        warn!(
                            "Discarding intent for {} ({} chars, expected 1..500)",
                            name,
                            intent.len()
                        );
                    }
                }
                Err(e) => {
                    warn!("Failed to generate intent for {}: {}", name, e);
                }
            }
        }
    }

    // Write resolved intents and their content hashes back to the definitions.
    let total = intents.len();
    for (name, intent) in intents {
        if let Some(def) = function_definitions.get_mut(&name) {
            def.intent = Some(intent);
            def.intent_input_hash = hashes.get(&name).cloned();
        }
    }

    debug!(
        "Intents: {} total ({} reused from content-hash cache, {} freshly generated)",
        total, reused, generated
    );

    // Strip body_source — source code stays in GitHub, not AWS
    strip_body_source(function_definitions);
}

/// Remove body_source from all function definitions.
/// The intent is the index; GitHub is the source of truth for code.
fn strip_body_source(function_definitions: &mut HashMap<String, FunctionDefinition>) {
    for def in function_definitions.values_mut() {
        def.body_source = None;
    }
}

/// Topological sort into parallel levels.
/// Level 0 = functions with no local deps (leaves).
/// Level 1 = functions whose deps are all in level 0. Etc.
/// Functions within the same level can run in parallel.
fn topological_levels(names: &[String], deps: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    let name_set: HashSet<&str> = names.iter().map(|s| s.as_str()).collect();
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut reverse_deps: HashMap<&str, Vec<&str>> = HashMap::new();

    for name in names {
        in_degree.entry(name.as_str()).or_insert(0);
        if let Some(called) = deps.get(name) {
            for callee in called {
                if name_set.contains(callee.as_str()) {
                    *in_degree.entry(name.as_str()).or_insert(0) += 1;
                    reverse_deps
                        .entry(callee.as_str())
                        .or_default()
                        .push(name.as_str());
                }
            }
        }
    }

    let mut levels: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<&str> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(&name, _)| name)
        .collect();

    while !current.is_empty() {
        levels.push(current.iter().map(|s| s.to_string()).collect());
        let mut next = Vec::new();
        for &name in &current {
            if let Some(dependents) = reverse_deps.get(name) {
                for &dep in dependents {
                    if let Some(deg) = in_degree.get_mut(dep) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            next.push(dep);
                        }
                    }
                }
            }
        }
        current = next;
    }

    // Add any remaining (cycles) as a final level
    let in_levels: HashSet<&str> = levels.iter().flatten().map(|s| s.as_str()).collect();
    let remaining: Vec<String> = names
        .iter()
        .filter(|n| !in_levels.contains(n.as_str()))
        .cloned()
        .collect();
    if !remaining.is_empty() {
        levels.push(remaining);
    }

    levels
}

// build_intent_prompt was moved to carrick-cloud/lambdas/generate-intent/index.js
// (buildPrompt). Rust now sends {name, body, called_intents} as a structured
// payload; the lambda assembles the prompt from those fields.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topological_levels_leaves_first() {
        let names = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut deps = HashMap::new();
        // c calls a and b, b calls a
        deps.insert("c".to_string(), vec!["a".to_string(), "b".to_string()]);
        deps.insert("b".to_string(), vec!["a".to_string()]);

        let levels = topological_levels(&names, &deps);
        assert!(levels.len() >= 2, "should have at least 2 levels");
        // Level 0 should contain "a" (leaf)
        assert!(
            levels[0].contains(&"a".to_string()),
            "a should be in level 0"
        );
        // "c" should be in a later level than "b"
        let b_level = levels
            .iter()
            .position(|l| l.contains(&"b".to_string()))
            .unwrap();
        let c_level = levels
            .iter()
            .position(|l| l.contains(&"c".to_string()))
            .unwrap();
        assert!(b_level < c_level, "b should be in an earlier level than c");
    }

    #[test]
    fn topological_levels_no_deps_single_level() {
        let names = vec!["x".to_string(), "y".to_string()];
        let deps = HashMap::new();
        let levels = topological_levels(&names, &deps);
        assert_eq!(levels.len(), 1, "all functions should be in one level");
        assert_eq!(levels[0].len(), 2);
    }

    #[test]
    fn topological_levels_handles_cycles() {
        let names = vec!["a".to_string(), "b".to_string()];
        let mut deps = HashMap::new();
        deps.insert("a".to_string(), vec!["b".to_string()]);
        deps.insert("b".to_string(), vec!["a".to_string()]);
        let levels = topological_levels(&names, &deps);
        let total: usize = levels.iter().map(|l| l.len()).sum();
        assert_eq!(total, 2, "both should still appear");
    }

    // build_prompt_without_deps and build_prompt_with_deps were removed:
    // prompt construction moved to /generate-intent lambda. Equivalent
    // behavioural test now lives in carrick-cloud (TBD).

    #[test]
    fn strip_body_source_removes_all() {
        let mut defs = HashMap::new();
        defs.insert(
            "foo".to_string(),
            FunctionDefinition {
                name: "foo".to_string(),
                file_path: "test.ts".into(),
                node_type: Default::default(),
                arguments: vec![],
                body_source: Some("return 1;".to_string()),
                is_exported: true,
                line_number: 1,
                intent: Some("returns one".to_string()),
                calls: vec![],
                return_type: None,
                return_is_explicit: false,
                signature: None,
                intent_input_hash: None,
            },
        );
        strip_body_source(&mut defs);
        assert!(defs.get("foo").unwrap().body_source.is_none());
        // Intent should be preserved
        assert!(defs.get("foo").unwrap().intent.is_some());
    }

    #[test]
    fn intent_hash_is_deterministic() {
        let called = vec!["- a: does a".to_string(), "- b: does b".to_string()];
        let h1 = compute_intent_hash("return 1;", &called);
        let h2 = compute_intent_hash("return 1;", &called);
        assert_eq!(h1, h2);
    }

    #[test]
    fn intent_hash_ignores_called_intents_order() {
        let a = vec!["- a: does a".to_string(), "- b: does b".to_string()];
        let b = vec!["- b: does b".to_string(), "- a: does a".to_string()];
        assert_eq!(
            compute_intent_hash("return 1;", &a),
            compute_intent_hash("return 1;", &b),
            "reordered callee intents must hash identically"
        );
    }

    #[test]
    fn intent_hash_changes_with_body() {
        let called: Vec<String> = vec![];
        assert_ne!(
            compute_intent_hash("return 1;", &called),
            compute_intent_hash("return 2;", &called)
        );
    }

    #[test]
    fn intent_hash_changes_when_callee_intent_changes() {
        // A caller whose callee's intent shifts must get a new hash so the
        // stale cached intent is regenerated rather than reused.
        let before = vec!["- helper: validates the token".to_string()];
        let after = vec!["- helper: parses the token".to_string()];
        assert_ne!(
            compute_intent_hash("return helper();", &before),
            compute_intent_hash("return helper();", &after)
        );
    }

    #[test]
    fn intents_by_hash_keeps_only_complete_entries() {
        let mut defs = HashMap::new();
        let base = FunctionDefinition {
            name: "f".to_string(),
            file_path: "test.ts".into(),
            node_type: Default::default(),
            arguments: vec![],
            body_source: None,
            is_exported: true,
            line_number: 1,
            intent: None,
            calls: vec![],
            return_type: None,
            return_is_explicit: false,
            signature: None,
            intent_input_hash: None,
        };

        // Complete: both intent and hash present → kept.
        defs.insert(
            "complete".to_string(),
            FunctionDefinition {
                intent: Some("does the thing".to_string()),
                intent_input_hash: Some("abc123".to_string()),
                ..base.clone()
            },
        );
        // Intent but no hash (pre-content-hash scan) → skipped.
        defs.insert(
            "no_hash".to_string(),
            FunctionDefinition {
                intent: Some("does another thing".to_string()),
                ..base.clone()
            },
        );
        // Hash but no intent (generation failed) → skipped.
        defs.insert(
            "no_intent".to_string(),
            FunctionDefinition {
                intent_input_hash: Some("def456".to_string()),
                ..base.clone()
            },
        );

        let map = intents_by_hash(&defs);
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get("abc123").map(String::as_str),
            Some("does the thing")
        );
    }

    fn def_with_body(name: &str, body: &str) -> FunctionDefinition {
        FunctionDefinition {
            name: name.to_string(),
            file_path: "test.ts".into(),
            node_type: Default::default(),
            arguments: vec![],
            body_source: Some(body.to_string()),
            is_exported: true,
            line_number: 1,
            intent: None,
            calls: vec![],
            return_type: None,
            return_is_explicit: false,
            signature: None,
            intent_input_hash: None,
        }
    }

    /// When every function's content hash is present in the previous-scan map,
    /// all intents are reused and NO `/generate-intent` call is made (the test
    /// would otherwise hit the network and fail). Also exercises the caller's
    /// hash composing its callee's resolved intent.
    #[tokio::test]
    async fn full_cache_hit_makes_no_lambda_calls() {
        // `main` calls `helper`; helper is the leaf (level 0).
        let mut defs = HashMap::new();
        defs.insert("helper".to_string(), def_with_body("helper", "return 1;"));
        defs.insert(
            "main".to_string(),
            def_with_body("main", "return helper();"),
        );

        // Reconstruct the exact hashes the generator will compute.
        let helper_intent = "returns the number one";
        let helper_hash = compute_intent_hash("return 1;", &[]);
        let caller_context = vec![format!("- helper: {}", helper_intent)];
        let main_hash = compute_intent_hash("return helper();", &caller_context);

        let mut prev = HashMap::new();
        prev.insert(helper_hash.clone(), helper_intent.to_string());
        prev.insert(main_hash.clone(), "calls the helper".to_string());

        let agent = AgentService::new();
        generate_function_intents(
            &agent,
            &mut defs,
            &HashMap::<String, ImportedSymbol>::new(),
            &prev,
        )
        .await;

        // Both intents came from the cache, with their hashes recorded.
        assert_eq!(defs["helper"].intent.as_deref(), Some(helper_intent));
        assert_eq!(defs["main"].intent.as_deref(), Some("calls the helper"));
        assert_eq!(
            defs["helper"].intent_input_hash.as_deref(),
            Some(helper_hash.as_str())
        );
        assert_eq!(
            defs["main"].intent_input_hash.as_deref(),
            Some(main_hash.as_str())
        );
        // body_source is stripped before upload.
        assert!(defs["helper"].body_source.is_none());
        assert!(defs["main"].body_source.is_none());
    }
}
