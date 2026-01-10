//! File-centric orchestrator for processing files using the FileAnalyzerAgent.
//!
//! NOTE: This module is part of a refactoring effort. The public API will be integrated
//! with the main orchestrator in subsequent commits.
#![allow(dead_code)]
//!
//! This orchestrator implements the AST-Gated File-Centric architecture:
//! 1. **Gatekeeper:** Run SWC Scanner to find potential API call sites
//! 2. **Check Relevance:** If no candidates found → SKIP file (Cost: $0)
//! 3. **Context:** Send Full File + Patterns + Candidate Targets to Gemini
//! 4. **Direct Build:** Deserialize JSON response directly into MountGraph structs
//!
//! This approach:
//! - Skips files with no API patterns (zero LLM cost)
//! - Utilizes Gemini's large context window for better alias resolution
//! - Passes AST-detected lines as "Candidate Targets" to ensure 100% recall
//! - Produces deterministic results through strict schema enforcement

use crate::{
    agent_service::AgentService,
    agents::{
        file_analyzer_agent::{FileAnalysisResult, FileAnalyzerAgent},
        framework_guidance_agent::FrameworkGuidance,
    },
    framework_detector::DetectionResult,
    mount_graph::{DataFetchingCall, GraphNode, MountEdge, MountGraph, NodeType, ResolvedEndpoint},
    swc_scanner::SwcScanner,
};
use std::collections::HashMap;
use std::path::PathBuf;

/// Complete result of file-centric analysis
#[derive(Debug)]
pub struct FileCentricAnalysisResult {
    /// Per-file analysis results
    pub file_results: HashMap<String, FileAnalysisResult>,
    /// Aggregated mount graph
    pub mount_graph: MountGraph,
    /// Processing statistics
    pub stats: ProcessingStats,
}

/// Statistics about the file-centric analysis
#[derive(Debug, Default)]
pub struct ProcessingStats {
    pub files_processed: usize,
    pub files_skipped: usize,
    /// Files skipped because SWC found no API candidates (zero-cost skips)
    pub files_skipped_no_candidates: usize,
    pub total_mounts: usize,
    pub total_endpoints: usize,
    pub total_data_calls: usize,
    pub errors: Vec<String>,
}

/// Orchestrates file-centric analysis using the FileAnalyzerAgent.
///
/// This orchestrator implements the AST-Gated architecture:
/// 1. **Gatekeeper:** Use SWC Scanner to find potential API call sites
/// 2. **Check Relevance:** If no candidates → skip file (zero cost)
/// 3. **Context:** Send Full File + Patterns + Candidate Targets to Gemini
/// 4. **Build:** Deserialize response directly into MountGraph
pub struct FileOrchestrator {
    file_analyzer: FileAnalyzerAgent,
    swc_scanner: SwcScanner,
}

impl FileOrchestrator {
    pub fn new(agent_service: AgentService) -> Self {
        Self {
            file_analyzer: FileAnalyzerAgent::new(agent_service),
            swc_scanner: SwcScanner::new(),
        }
    }

    /// Run AST-gated file-centric analysis on all provided files.
    ///
    /// **AST-Gated Architecture:**
    /// 1. Run SWC Scanner on each file to find potential API call sites
    /// 2. If no candidates found → SKIP file (zero LLM cost)
    /// 3. If candidates exist → Send Full File + Patterns + Candidate Hints to Gemini
    /// 4. Merge results into MountGraph
    ///
    /// # Arguments
    /// * `files` - List of file paths to analyze
    /// * `guidance` - Framework-specific patterns for detection
    /// * `_framework_detection` - Framework detection results (for future use)
    ///
    /// # Returns
    /// A `FileCentricAnalysisResult` containing per-file results and aggregated graph.
    pub async fn analyze_files(
        &self,
        files: &[PathBuf],
        guidance: &FrameworkGuidance,
        _framework_detection: &DetectionResult,
    ) -> Result<FileCentricAnalysisResult, Box<dyn std::error::Error>> {
        println!("=== AST-GATED FILE-CENTRIC ORCHESTRATOR ===");
        println!("Processing {} files with SWC gatekeeper", files.len());

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        let mut stats = ProcessingStats::default();

        // Process each file with AST gatekeeper
        for file_path in files {
            let path_str = file_path.to_string_lossy().to_string();

            // Read file content
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(e) => {
                    stats
                        .errors
                        .push(format!("Failed to read {}: {}", path_str, e));
                    stats.files_skipped += 1;
                    continue;
                }
            };

            // Skip empty files
            if content.trim().is_empty() {
                println!("Skipping empty file: {}", path_str);
                stats.files_skipped += 1;
                continue;
            }

            // STEP 1: Run SWC Scanner (Gatekeeper)
            let scan_result = self.swc_scanner.scan_content(file_path, &content);

            // STEP 2: Check Relevance - if no candidates, SKIP (zero LLM cost)
            if !scan_result.should_analyze {
                println!("Skipped (no API patterns): {} [0 candidates]", path_str);
                stats.files_skipped += 1;
                stats.files_skipped_no_candidates += 1;
                continue;
            }

            println!(
                "Analyzing: {} [{} candidates detected by SWC]",
                path_str,
                scan_result.candidates.len()
            );

            // STEP 3: Prepare Candidate Targets as hints for the LLM
            let candidate_hints: Vec<String> = scan_result
                .candidates
                .iter()
                .map(|c| c.format_hint())
                .collect();

            // STEP 4: Call Gemini with Full File + Patterns + Candidate Targets
            match self
                .file_analyzer
                .analyze_file_with_candidates(&path_str, &content, guidance, &candidate_hints)
                .await
            {
                Ok(result) => {
                    stats.total_mounts += result.mounts.len();
                    stats.total_endpoints += result.endpoints.len();
                    stats.total_data_calls += result.data_calls.len();
                    stats.files_processed += 1;
                    file_results.insert(path_str, result);
                }
                Err(e) => {
                    stats
                        .errors
                        .push(format!("Failed to analyze {}: {}", path_str, e));
                    stats.files_skipped += 1;
                }
            }
        }

        println!("\n=== FILE PROCESSING COMPLETE ===");
        println!("  - Files processed (LLM calls): {}", stats.files_processed);
        println!("  - Files skipped (total): {}", stats.files_skipped);
        println!(
            "  - Zero-cost skips (no API patterns): {}",
            stats.files_skipped_no_candidates
        );
        println!("  - Total mounts: {}", stats.total_mounts);
        println!("  - Total endpoints: {}", stats.total_endpoints);
        println!("  - Total data calls: {}", stats.total_data_calls);

        // STEP 5: Build aggregated mount graph from all file results
        let mount_graph = self.build_mount_graph(&file_results);

        Ok(FileCentricAnalysisResult {
            file_results,
            mount_graph,
            stats,
        })
    }

    /// Build a MountGraph from aggregated file analysis results.
    ///
    /// This implements the key insight from the refactoring plan:
    /// The `import_source` field from each mount result is the key to cross-file resolution.
    fn build_mount_graph(&self, file_results: &HashMap<String, FileAnalysisResult>) -> MountGraph {
        let mut graph = MountGraph::new();

        // Track import mappings: import_source -> (file_path, local_name)
        let mut import_map: HashMap<String, String> = HashMap::new();

        // First pass: collect all nodes and build import mappings
        for (file_path, result) in file_results {
            // Add nodes from endpoints
            for endpoint in &result.endpoints {
                let node_key = format!("{}:{}", file_path, endpoint.owner_node);
                if !graph.nodes.contains_key(&node_key) {
                    graph.nodes.insert(
                        endpoint.owner_node.clone(),
                        GraphNode {
                            name: endpoint.owner_node.clone(),
                            node_type: NodeType::Unknown,
                            creation_site: None,
                            file_location: format!("{}:{}", file_path, endpoint.line_number),
                        },
                    );
                }
            }

            // Add nodes and import mappings from mounts
            for mount in &result.mounts {
                // Add parent node
                if !graph.nodes.contains_key(&mount.parent_node) {
                    graph.nodes.insert(
                        mount.parent_node.clone(),
                        GraphNode {
                            name: mount.parent_node.clone(),
                            node_type: NodeType::Unknown,
                            creation_site: None,
                            file_location: format!("{}:{}", file_path, mount.line_number),
                        },
                    );
                }

                // Add child node
                if !graph.nodes.contains_key(&mount.child_node) {
                    graph.nodes.insert(
                        mount.child_node.clone(),
                        GraphNode {
                            name: mount.child_node.clone(),
                            node_type: NodeType::Unknown,
                            creation_site: None,
                            file_location: format!("{}:{}", file_path, mount.line_number),
                        },
                    );
                }

                // Track import source for cross-file resolution
                if let Some(import_source) = &mount.import_source {
                    // Normalize the import source
                    let normalized = Self::normalize_import_source(import_source);
                    import_map.insert(normalized, mount.child_node.clone());
                }
            }
        }

        // Second pass: build mount edges with resolved names
        for (file_path, result) in file_results {
            for mount in &result.mounts {
                graph.mounts.push(MountEdge {
                    parent: mount.parent_node.clone(),
                    child: mount.child_node.clone(),
                    path_prefix: mount.mount_path.clone(),
                    middleware_stack: Vec::new(),
                });

                // Store import mapping for later endpoint resolution
                if let Some(import_source) = &mount.import_source {
                    let normalized = Self::normalize_import_source(import_source);
                    graph.nodes.insert(
                        format!("__import_map__::{}", normalized),
                        GraphNode {
                            name: mount.child_node.clone(),
                            node_type: NodeType::Unknown,
                            creation_site: None,
                            file_location: file_path.clone(),
                        },
                    );
                }
            }
        }

        // Third pass: infer node types based on mount behavior
        self.infer_node_types(&mut graph);

        // Fourth pass: add endpoints with resolved owners
        for (file_path, result) in file_results {
            for endpoint in &result.endpoints {
                // Try to resolve the owner using import information
                let resolved_owner =
                    self.resolve_endpoint_owner(&graph, &endpoint.owner_node, file_path);

                graph.endpoints.push(ResolvedEndpoint {
                    method: endpoint.method.clone(),
                    path: endpoint.path.clone(),
                    full_path: endpoint.path.clone(), // Will be resolved later
                    handler: Some(endpoint.handler_name.clone()),
                    owner: resolved_owner,
                    file_location: format!("{}:{}", file_path, endpoint.line_number),
                    middleware_chain: Vec::new(),
                    repo_name: None,
                });
            }
        }

        // Fifth pass: add data calls
        for (file_path, result) in file_results {
            for data_call in &result.data_calls {
                graph.data_calls.push(DataFetchingCall {
                    method: data_call
                        .method
                        .clone()
                        .unwrap_or_else(|| "GET".to_string()),
                    target_url: data_call.target.clone(),
                    client: data_call.pattern_matched.clone(),
                    file_location: format!("{}:{}", file_path, data_call.line_number),
                });
            }
        }

        // Sixth pass: resolve full paths for endpoints
        self.resolve_endpoint_paths(&mut graph);

        graph
    }

    /// Normalize import source paths for matching.
    fn normalize_import_source(source: &str) -> String {
        source
            .trim_start_matches("./")
            .trim_start_matches("../")
            .trim_end_matches(".ts")
            .trim_end_matches(".js")
            .trim_end_matches(".tsx")
            .trim_end_matches(".jsx")
            .to_string()
    }

    /// Infer node types based on mount behavior.
    fn infer_node_types(&self, graph: &mut MountGraph) {
        // Nodes that are mounted by others are Mountable
        let mounted_children: std::collections::HashSet<_> =
            graph.mounts.iter().map(|m| m.child.clone()).collect();

        // Nodes that mount others are potential Roots
        let mounting_parents: std::collections::HashSet<_> =
            graph.mounts.iter().map(|m| m.parent.clone()).collect();

        for (name, node) in graph.nodes.iter_mut() {
            if name.starts_with("__import_map__") {
                continue;
            }

            if mounted_children.contains(name) {
                node.node_type = NodeType::Mountable;
            } else if mounting_parents.contains(name) && !mounted_children.contains(name) {
                node.node_type = NodeType::Root;
            }
        }
    }

    /// Resolve endpoint owner using import information.
    fn resolve_endpoint_owner(
        &self,
        graph: &MountGraph,
        owner_name: &str,
        file_path: &str,
    ) -> String {
        // Extract just the filename parts for matching
        let file_parts: Vec<&str> = file_path.split('/').collect();

        // Try to find a matching import mapping
        for (key, node) in &graph.nodes {
            if key.starts_with("__import_map__::") {
                let source_pattern = key.trim_start_matches("__import_map__::");

                // Check if the current file matches this source pattern
                if file_path.contains(source_pattern)
                    || file_parts.iter().any(|part| part.contains(source_pattern))
                {
                    return node.name.clone();
                }
            }
        }

        // No mapping found, return original owner
        owner_name.to_string()
    }

    /// Resolve full paths for endpoints by traversing the mount graph.
    fn resolve_endpoint_paths(&self, graph: &mut MountGraph) {
        // Build owner -> mount path prefix map
        let mut owner_prefixes: HashMap<String, String> = HashMap::new();

        // Traverse mounts to build path prefixes
        for mount in &graph.mounts {
            let existing = owner_prefixes
                .get(&mount.child)
                .map(|s| s.as_str())
                .unwrap_or("");
            let new_prefix = format!("{}{}", existing, mount.path_prefix);
            owner_prefixes.insert(mount.child.clone(), new_prefix);
        }

        // Apply prefixes to endpoints
        for endpoint in &mut graph.endpoints {
            if let Some(prefix) = owner_prefixes.get(&endpoint.owner) {
                endpoint.full_path = format!("{}{}", prefix, endpoint.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::file_analyzer_agent::{DataCallResult, EndpointResult, MountResult};

    #[test]
    fn test_normalize_import_source() {
        assert_eq!(
            FileOrchestrator::normalize_import_source("./routes/users"),
            "routes/users"
        );
        assert_eq!(
            FileOrchestrator::normalize_import_source("../api/index.ts"),
            "api/index"
        );
        assert_eq!(
            FileOrchestrator::normalize_import_source("./auth.js"),
            "auth"
        );
        assert_eq!(
            FileOrchestrator::normalize_import_source("components/Header.tsx"),
            "components/Header"
        );
    }

    #[test]
    fn test_build_mount_graph_from_single_file() {
        let agent_service = AgentService::new("mock".to_string());
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();
        file_results.insert(
            "src/app.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![MountResult {
                    line_number: 10,
                    parent_node: "app".to_string(),
                    child_node: "userRouter".to_string(),
                    mount_path: "/users".to_string(),
                    import_source: Some("./routes/users".to_string()),
                    pattern_matched: ".use(".to_string(),
                }],
                endpoints: vec![EndpointResult {
                    line_number: 5,
                    owner_node: "app".to_string(),
                    method: "GET".to_string(),
                    path: "/health".to_string(),
                    handler_name: "healthCheck".to_string(),
                    pattern_matched: ".get(".to_string(),
                }],
                data_calls: vec![],
            },
        );

        let graph = orchestrator.build_mount_graph(&file_results);

        assert_eq!(graph.mounts.len(), 1);
        assert_eq!(graph.endpoints.len(), 1);
        assert_eq!(graph.mounts[0].parent, "app");
        assert_eq!(graph.mounts[0].child, "userRouter");
        assert_eq!(graph.mounts[0].path_prefix, "/users");
    }

    #[test]
    fn test_build_mount_graph_with_data_calls() {
        let agent_service = AgentService::new("mock".to_string());
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();
        file_results.insert(
            "src/service.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![DataCallResult {
                    line_number: 15,
                    target: "https://api.example.com/data".to_string(),
                    method: Some("POST".to_string()),
                    pattern_matched: "fetch(".to_string(),
                }],
            },
        );

        let graph = orchestrator.build_mount_graph(&file_results);

        assert_eq!(graph.data_calls.len(), 1);
        assert_eq!(
            graph.data_calls[0].target_url,
            "https://api.example.com/data"
        );
        assert_eq!(graph.data_calls[0].method, "POST");
    }

    #[test]
    fn test_build_mount_graph_cross_file_resolution() {
        let agent_service = AgentService::new("mock".to_string());
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();

        // Main app file that imports and mounts user router
        file_results.insert(
            "src/app.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![MountResult {
                    line_number: 10,
                    parent_node: "app".to_string(),
                    child_node: "userRouter".to_string(),
                    mount_path: "/api/users".to_string(),
                    import_source: Some("./routes/users".to_string()),
                    pattern_matched: ".use(".to_string(),
                }],
                endpoints: vec![],
                data_calls: vec![],
            },
        );

        // User routes file with endpoints
        file_results.insert(
            "src/routes/users.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![
                    EndpointResult {
                        line_number: 5,
                        owner_node: "router".to_string(),
                        method: "GET".to_string(),
                        path: "/".to_string(),
                        handler_name: "listUsers".to_string(),
                        pattern_matched: ".get(".to_string(),
                    },
                    EndpointResult {
                        line_number: 10,
                        owner_node: "router".to_string(),
                        method: "POST".to_string(),
                        path: "/".to_string(),
                        handler_name: "createUser".to_string(),
                        pattern_matched: ".post(".to_string(),
                    },
                ],
                data_calls: vec![],
            },
        );

        let graph = orchestrator.build_mount_graph(&file_results);

        // Should have the mount and both endpoints
        assert_eq!(graph.mounts.len(), 1);
        assert_eq!(graph.endpoints.len(), 2);

        // Verify the import mapping was created
        let has_import_map = graph
            .nodes
            .keys()
            .any(|k| k.starts_with("__import_map__::"));
        assert!(has_import_map, "Should have import mapping node");
    }

    #[test]
    fn test_infer_node_types() {
        let agent_service = AgentService::new("mock".to_string());
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut graph = MountGraph::new();

        // Add nodes
        graph.nodes.insert(
            "app".to_string(),
            GraphNode {
                name: "app".to_string(),
                node_type: NodeType::Unknown,
                creation_site: None,
                file_location: "app.ts:1".to_string(),
            },
        );
        graph.nodes.insert(
            "userRouter".to_string(),
            GraphNode {
                name: "userRouter".to_string(),
                node_type: NodeType::Unknown,
                creation_site: None,
                file_location: "routes/users.ts:1".to_string(),
            },
        );

        // Add mount: app mounts userRouter
        graph.mounts.push(MountEdge {
            parent: "app".to_string(),
            child: "userRouter".to_string(),
            path_prefix: "/users".to_string(),
            middleware_stack: vec![],
        });

        orchestrator.infer_node_types(&mut graph);

        // app should be Root (mounts others, not mounted)
        assert_eq!(graph.nodes.get("app").unwrap().node_type, NodeType::Root);
        // userRouter should be Mountable (is mounted)
        assert_eq!(
            graph.nodes.get("userRouter").unwrap().node_type,
            NodeType::Mountable
        );
    }

    #[test]
    fn test_processing_stats_default() {
        let stats = ProcessingStats::default();
        assert_eq!(stats.files_processed, 0);
        assert_eq!(stats.files_skipped, 0);
        assert_eq!(stats.total_mounts, 0);
        assert_eq!(stats.total_endpoints, 0);
        assert_eq!(stats.total_data_calls, 0);
        assert!(stats.errors.is_empty());
    }
}
