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
    url_normalizer::UrlNormalizer,
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
        let normalizer = UrlNormalizer::default_permissive();
        let normalized_endpoint = normalizer.normalize(endpoint_path);
        let normalized_call = normalizer.normalize(call_url);

        if normalized_call.is_external {
            return false;
        }

        self.paths_match(&normalized_endpoint.path, &normalized_call.path)
    }

    fn paths_match(&self, endpoint_path: &str, call_path: &str) -> bool {
        if endpoint_path == call_path {
            return true;
        }

        if self.path_matches_with_params(endpoint_path, call_path) {
            return true;
        }

        self.path_matches_with_wildcards(endpoint_path, call_path)
    }

    fn path_matches_with_params(&self, endpoint_path: &str, call_path: &str) -> bool {
        let endpoint_segments: Vec<&str> = endpoint_path.split('/').collect();
        let call_segments: Vec<&str> = call_path.split('/').collect();

        let endpoint_required_count = endpoint_segments
            .iter()
            .filter(|s| !s.ends_with('?'))
            .count();

        if call_segments.len() < endpoint_required_count
            || call_segments.len() > endpoint_segments.len()
        {
            return false;
        }

        for (i, endpoint_seg) in endpoint_segments.iter().enumerate() {
            let is_optional = endpoint_seg.ends_with('?');
            let seg = endpoint_seg.trim_end_matches('?');

            if i >= call_segments.len() {
                if !is_optional {
                    return false;
                }
                continue;
            }

            let call_seg = call_segments[i];

            if seg.starts_with(':') {
                continue;
            }

            if seg != call_seg {
                return false;
            }
        }

        true
    }

    fn path_matches_with_wildcards(&self, endpoint_path: &str, call_path: &str) -> bool {
        if endpoint_path.ends_with("/*") || endpoint_path.ends_with("/**") {
            let prefix = endpoint_path.trim_end_matches("/**").trim_end_matches("/*");
            return call_path.starts_with(prefix);
        }

        if endpoint_path.ends_with("/(.*)") {
            let prefix = endpoint_path.trim_end_matches("/(.*)");
            return call_path.starts_with(prefix);
        }

        let endpoint_segments: Vec<&str> = endpoint_path.split('/').collect();
        let call_segments: Vec<&str> = call_path.split('/').collect();

        if endpoint_segments.len() != call_segments.len() {
            return false;
        }

        for (endpoint_seg, call_seg) in endpoint_segments.iter().zip(call_segments.iter()) {
            if *endpoint_seg == "*" {
                continue;
            }
            if endpoint_seg != call_seg {
                return false;
            }
        }

        true
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
