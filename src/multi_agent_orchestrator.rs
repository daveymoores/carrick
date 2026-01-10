//! Multi-Agent Orchestrator using AST-Gated File-Centric Analysis.
//!
//! This orchestrator implements the new analysis workflow:
//! 1. Framework Detection - Identify frameworks and data fetchers
//! 2. Framework Guidance Generation - Get patterns for detected frameworks
//! 3. AST-Gated File Analysis - Use SWC Scanner as gatekeeper, then LLM for relevant files
//! 4. Mount Graph Construction - Build from file analysis results
//!
//! The AST-Gated approach:
//! - Skips files with no API patterns (zero LLM cost)
//! - Sends full file context for better alias resolution
//! - Passes AST-detected candidate hints for 100% recall

use crate::{
    agent_service::AgentService,
    agents::{
        file_analyzer_agent::FileAnalysisResult,
        file_orchestrator::{FileCentricAnalysisResult, FileOrchestrator, ProcessingStats},
        framework_guidance_agent::{FrameworkGuidance, FrameworkGuidanceAgent},
    },
    framework_detector::{DetectionResult, FrameworkDetector},
    mount_graph::MountGraph,
    packages::Packages,
    visitor::ImportedSymbol,
};
use std::collections::HashMap;
use swc_common::{SourceMap, sync::Lrc};

/// Complete analysis result from the multi-agent workflow
#[derive(Debug)]
pub struct MultiAgentAnalysisResult {
    #[allow(dead_code)]
    pub framework_detection: DetectionResult,
    #[allow(dead_code)]
    pub framework_guidance: FrameworkGuidance,
    pub mount_graph: MountGraph,
    /// File-centric analysis results (replaces old AnalysisResults)
    pub file_results: HashMap<String, FileAnalysisResult>,
    /// Processing statistics
    #[allow(dead_code)]
    pub stats: ProcessingStats,
}

/// Orchestrates the complete multi-agent workflow using AST-Gated File-Centric analysis.
///
/// This orchestrator:
/// 1. Detects frameworks from package.json and imports
/// 2. Generates framework-specific patterns via LLM
/// 3. Uses SWC Scanner as gatekeeper to skip irrelevant files (zero cost)
/// 4. Sends relevant files to LLM with full context + candidate hints
/// 5. Builds MountGraph from aggregated results
pub struct MultiAgentOrchestrator {
    agent_service: AgentService,
    #[allow(dead_code)]
    source_map: Lrc<SourceMap>,
}

impl MultiAgentOrchestrator {
    pub fn new(api_key: String, source_map: Lrc<SourceMap>) -> Self {
        Self {
            agent_service: AgentService::new(api_key),
            source_map,
        }
    }

    /// Run the complete multi-agent analysis workflow using AST-Gated File-Centric approach.
    ///
    /// ## Workflow:
    /// 1. **Framework Detection** - Identify frameworks from packages and imports
    /// 2. **Framework Guidance** - Generate patterns for detected frameworks
    /// 3. **AST-Gated Analysis** - For each file:
    ///    - Run SWC Scanner to find candidates
    ///    - If no candidates → SKIP (zero LLM cost)
    ///    - If candidates exist → Send full file + patterns + hints to LLM
    /// 4. **Graph Construction** - Build MountGraph from all file results
    pub async fn run_complete_analysis(
        &self,
        files: Vec<std::path::PathBuf>,
        packages: &Packages,
        imported_symbols: &HashMap<String, ImportedSymbol>,
    ) -> Result<MultiAgentAnalysisResult, Box<dyn std::error::Error>> {
        println!("Starting AST-Gated File-Centric analysis...");

        // Stage 0: Framework Detection
        println!("\n=== Stage 0: Framework Detection ===");
        let framework_detector = FrameworkDetector::new(self.agent_service.clone());
        let framework_detection = framework_detector
            .detect_frameworks_and_libraries(packages, imported_symbols)
            .await?;

        println!("Detected frameworks: {:?}", framework_detection.frameworks);
        println!(
            "Detected data fetchers: {:?}",
            framework_detection.data_fetchers
        );

        // Stage 1: Framework Guidance Generation
        println!("\n=== Stage 1: Framework Guidance Generation ===");
        let framework_guidance_agent = FrameworkGuidanceAgent::new(self.agent_service.clone());
        let framework_guidance = framework_guidance_agent
            .generate_guidance(&framework_detection)
            .await?;

        println!(
            "Generated guidance with {} mount patterns, {} endpoint patterns, {} data fetching patterns",
            framework_guidance.mount_patterns.len(),
            framework_guidance.endpoint_patterns.len(),
            framework_guidance.data_fetching_patterns.len()
        );

        // Stage 2: AST-Gated File-Centric Analysis
        println!("\n=== Stage 2: AST-Gated File-Centric Analysis ===");
        let file_orchestrator = FileOrchestrator::new(self.agent_service.clone());
        let file_centric_result = file_orchestrator
            .analyze_files(&files, &framework_guidance, &framework_detection)
            .await?;

        self.print_analysis_summary(&file_centric_result);

        // Stage 3: Mount Graph is already built by FileOrchestrator
        println!("\n=== Stage 3: Mount Graph Summary ===");
        let mount_graph = &file_centric_result.mount_graph;
        println!("Built mount graph:");
        println!("  - {} nodes", mount_graph.get_nodes().len());
        println!("  - {} mounts", mount_graph.get_mounts().len());
        println!(
            "  - {} endpoints",
            mount_graph.get_resolved_endpoints().len()
        );
        println!("  - {} data calls", mount_graph.get_data_calls().len());

        Ok(MultiAgentAnalysisResult {
            framework_detection,
            framework_guidance,
            mount_graph: file_centric_result.mount_graph,
            file_results: file_centric_result.file_results,
            stats: file_centric_result.stats,
        })
    }

    fn print_analysis_summary(&self, result: &FileCentricAnalysisResult) {
        println!("\n=== ANALYSIS SUMMARY ===");
        println!("File Processing:");
        println!(
            "  - Files processed (LLM calls): {}",
            result.stats.files_processed
        );
        println!("  - Files skipped (total): {}", result.stats.files_skipped);
        println!(
            "  - Zero-cost skips (no API patterns): {}",
            result.stats.files_skipped_no_candidates
        );

        println!("Extracted Items:");
        println!("  - Total mounts: {}", result.stats.total_mounts);
        println!("  - Total endpoints: {}", result.stats.total_endpoints);
        println!("  - Total data calls: {}", result.stats.total_data_calls);

        if !result.stats.errors.is_empty() {
            println!("Errors ({}):", result.stats.errors.len());
            for error in &result.stats.errors {
                println!("  - {}", error);
            }
        }
    }

    /// Extract type information from file analysis results for type checking.
    ///
    /// This method extracts type positions from the file analysis results
    /// for use in cross-repo type checking.
    pub fn extract_types_from_file_results(
        &self,
        file_results: &HashMap<String, FileAnalysisResult>,
    ) -> Vec<serde_json::Value> {
        use crate::analyzer::Analyzer;
        let mut type_infos = Vec::new();

        println!("=== EXTRACT TYPES FROM FILE ANALYSIS ===");

        let mut total_endpoints = 0;
        let mut total_data_calls = 0;

        for (file_path, result) in file_results {
            total_endpoints += result.endpoints.len();
            total_data_calls += result.data_calls.len();

            // Extract types from endpoints
            for endpoint in &result.endpoints {
                let alias = Analyzer::generate_common_type_alias_name(
                    &endpoint.path,
                    &endpoint.method,
                    false, // is_request_type
                    false, // is_consumer
                );

                // Use the response_type_file from the LLM if available, otherwise use the current file
                let type_file = endpoint
                    .response_type_file
                    .clone()
                    .unwrap_or_else(|| file_path.clone());

                // Build type info with response type information if available
                let mut type_info = serde_json::json!({
                    "filePath": type_file,
                    "lineNumber": endpoint.line_number,
                    "alias": alias,
                    "kind": "endpoint",
                    "method": endpoint.method,
                    "path": endpoint.path
                });

                // Add response type position if available
                if let Some(pos) = endpoint.response_type_position {
                    type_info["startPosition"] = serde_json::json!(pos);
                }

                // Add the type string if the LLM extracted it
                if let Some(ref type_str) = endpoint.response_type_string {
                    type_info["compositeTypeString"] = serde_json::json!(type_str);
                }

                type_infos.push(type_info);
            }

            // Extract types from data calls
            for data_call in &result.data_calls {
                let alias = if let Some(method) = &data_call.method {
                    Analyzer::generate_common_type_alias_name(
                        &data_call.target,
                        method,
                        false, // is_request_type
                        true,  // is_consumer
                    )
                } else {
                    format!("DataCall_L{}", data_call.line_number)
                };

                // Use the response_type_file from the LLM if available, otherwise use the current file
                let type_file = data_call
                    .response_type_file
                    .clone()
                    .unwrap_or_else(|| file_path.clone());

                let mut type_info = serde_json::json!({
                    "filePath": type_file,
                    "lineNumber": data_call.line_number,
                    "alias": alias,
                    "kind": "data_call",
                    "target": data_call.target
                });

                // Add response type position if available
                if let Some(pos) = data_call.response_type_position {
                    type_info["startPosition"] = serde_json::json!(pos);
                }

                // Add the type string if the LLM extracted it
                if let Some(ref type_str) = data_call.response_type_string {
                    type_info["compositeTypeString"] = serde_json::json!(type_str);
                }

                type_infos.push(type_info);
            }
        }

        println!(
            "Processed {} endpoints and {} data calls from {} files",
            total_endpoints,
            total_data_calls,
            file_results.len()
        );
        println!("Extracted {} type infos", type_infos.len());
        println!("=== END EXTRACT TYPES ===");

        type_infos
    }
}

/// Analysis result in a format compatible with existing systems
#[derive(Debug)]
#[allow(dead_code)]
pub struct EndpointAnalysis {
    pub producers: Vec<crate::mount_graph::ResolvedEndpoint>,
    pub consumers: Vec<crate::mount_graph::DataFetchingCall>,
    pub mount_relationships: Vec<crate::mount_graph::MountEdge>,
}

#[allow(dead_code)]
impl EndpointAnalysis {
    /// Find potential API mismatches by comparing producers and consumers
    pub fn find_potential_mismatches(&self) -> Vec<PotentialMismatch> {
        let mut mismatches = Vec::new();

        for consumer in &self.consumers {
            let matching_producers: Vec<_> = self
                .producers
                .iter()
                .filter(|producer| {
                    producer.method.eq_ignore_ascii_case(&consumer.method)
                        && self.urls_could_match(&producer.full_path, &consumer.target_url)
                })
                .collect();

            if matching_producers.is_empty() {
                mismatches.push(PotentialMismatch {
                    consumer_call: consumer.clone(),
                    issue: MismatchType::MissingEndpoint,
                    details: format!(
                        "No producer found for {} {}",
                        consumer.method, consumer.target_url
                    ),
                });
            }
        }

        // Find orphaned endpoints
        for producer in &self.producers {
            let has_consumers = self.consumers.iter().any(|consumer| {
                consumer.method.eq_ignore_ascii_case(&producer.method)
                    && self.urls_could_match(&producer.full_path, &consumer.target_url)
            });

            if !has_consumers {
                mismatches.push(PotentialMismatch {
                    consumer_call: crate::mount_graph::DataFetchingCall {
                        method: producer.method.clone(),
                        target_url: producer.full_path.clone(),
                        client: "none".to_string(),
                        file_location: "orphaned".to_string(),
                    },
                    issue: MismatchType::OrphanedEndpoint,
                    details: format!(
                        "Endpoint {} {} has no consumers",
                        producer.method, producer.full_path
                    ),
                });
            }
        }

        mismatches
    }

    fn urls_could_match(&self, endpoint_path: &str, call_url: &str) -> bool {
        // Simple heuristic - could be enhanced
        if call_url.starts_with("http") {
            // Extract path from full URL
            if let Some(url_path) = call_url
                .split('/')
                .skip(3) // Skip protocol and domain
                .collect::<Vec<_>>()
                .first()
            {
                return endpoint_path.contains(url_path);
            }
        }

        endpoint_path == call_url || call_url.contains(endpoint_path)
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PotentialMismatch {
    pub consumer_call: crate::mount_graph::DataFetchingCall,
    pub issue: MismatchType,
    pub details: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum MismatchType {
    MissingEndpoint,
    OrphanedEndpoint,
    MethodMismatch,
    PathMismatch,
}
