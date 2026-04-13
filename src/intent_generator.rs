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
use std::collections::{HashMap, HashSet};
use tracing::{debug, warn};

/// Generate intents for all exported functions that have body source.
///
/// After generation:
/// - Each function's `intent` is populated with a 1-2 sentence description
/// - Each function's `calls` is populated with references to local callees
/// - `body_source` is stripped from ALL functions (source stays in GitHub, not AWS)
pub async fn generate_function_intents(
    agent_service: &AgentService,
    function_definitions: &mut HashMap<String, FunctionDefinition>,
    _imported_symbols: &HashMap<String, ImportedSymbol>,
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
        if let Some(def) = function_definitions.get(name) {
            if let Some(ref body) = def.body_source {
                let called: Vec<String> = local_fn_names
                    .iter()
                    .filter(|&&fn_name| fn_name != name.as_str() && body.contains(fn_name))
                    .map(|&s| s.to_string())
                    .collect();
                deps.insert(name.clone(), called);
            }
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

    // Generate intents level by level — within each level, calls run in parallel
    let mut intents: HashMap<String, String> = HashMap::new();
    let system_msg = "You describe what functions do in 1-2 sentences. Be specific about the business logic, not the implementation details. Respond with ONLY the description, no quotes or prefixes.";

    for level in &levels {
        // Build prompts for all functions in this level
        let tasks: Vec<(String, String)> = level
            .iter()
            .filter_map(|name| {
                let def = function_definitions.get(name)?;
                let body = def.body_source.as_ref()?;

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

                let prompt = build_intent_prompt(name, body, &called_intents);
                Some((name.clone(), prompt))
            })
            .collect();

        // Run all LLM calls for this level in parallel
        let futures: Vec<_> = tasks
            .iter()
            .map(|(name, prompt)| async {
                let result = agent_service.analyze_code(prompt, system_msg).await;
                (name.clone(), result)
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        for (name, result) in results {
            match result {
                Ok(intent) => {
                    let intent = intent.trim().to_string();
                    if !intent.is_empty() && intent.len() < 500 {
                        intents.insert(name, intent);
                    }
                }
                Err(e) => {
                    warn!("Failed to generate intent for {}: {}", name, e);
                }
            }
        }
    }

    // Write intents back to function definitions
    let count = intents.len();
    for (name, intent) in intents {
        if let Some(def) = function_definitions.get_mut(&name) {
            def.intent = Some(intent);
        }
    }

    debug!("Generated {} intent(s)", count);

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

fn build_intent_prompt(name: &str, body: &str, called_intents: &[String]) -> String {
    let mut prompt = String::new();

    if !called_intents.is_empty() {
        prompt.push_str("This function uses the following helper functions:\n");
        for intent in called_intents {
            prompt.push_str(intent);
            prompt.push('\n');
        }
        prompt.push('\n');
    }

    prompt.push_str(&format!(
        "Function `{}`:\n```\n{}\n```\n\nWhat does this function intend to do?",
        name, body
    ));
    prompt
}

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

    #[test]
    fn build_prompt_without_deps() {
        let prompt = build_intent_prompt("foo", "return 1 + 2;", &[]);
        assert!(prompt.contains("Function `foo`"));
        assert!(prompt.contains("return 1 + 2"));
        assert!(!prompt.contains("helper functions"));
    }

    #[test]
    fn build_prompt_with_deps() {
        let called = vec!["- validate: Checks email format".to_string()];
        let prompt =
            build_intent_prompt("createUser", "validate(email); db.insert(user);", &called);
        assert!(prompt.contains("helper functions"));
        assert!(prompt.contains("validate: Checks email format"));
        assert!(prompt.contains("Function `createUser`"));
    }

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
                end_line_number: 1,
                intent: Some("returns one".to_string()),
                calls: vec![],
            },
        );
        strip_body_source(&mut defs);
        assert!(defs.get("foo").unwrap().body_source.is_none());
        // Intent should be preserved
        assert!(defs.get("foo").unwrap().intent.is_some());
    }
}
