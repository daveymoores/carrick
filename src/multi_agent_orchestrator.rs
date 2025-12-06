use crate::{
    agents::{AnalysisResults, CallSiteOrchestrator},
    call_site_extractor::{CallSiteExtractionService, CallSiteExtractor},
    framework_detector::{DetectionResult, FrameworkDetector},
    gemini_service::GeminiService,
    mount_graph::MountGraph,
    packages::Packages,
    parser::parse_file,
    visitor::ImportedSymbol,
};
use std::collections::HashMap;
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};

/// Complete analysis result from the multi-agent workflow
#[derive(Debug)]
pub struct MultiAgentAnalysisResult {
    #[allow(dead_code)]
    pub framework_detection: DetectionResult,
    pub mount_graph: MountGraph,
    pub analysis_results: AnalysisResults,
}

/// Orchestrates the complete multi-agent workflow
pub struct MultiAgentOrchestrator {
    gemini_service: GeminiService,
    source_map: Lrc<SourceMap>,
}

impl MultiAgentOrchestrator {
    pub fn new(api_key: String, source_map: Lrc<SourceMap>) -> Self {
        Self {
            gemini_service: GeminiService::new(api_key),
            source_map,
        }
    }

    /// Run the complete multi-agent analysis workflow
    pub async fn run_complete_analysis(
        &self,
        files: Vec<std::path::PathBuf>,
        packages: &Packages,
        imported_symbols: &HashMap<String, ImportedSymbol>,
    ) -> Result<MultiAgentAnalysisResult, Box<dyn std::error::Error>> {
        println!("Starting multi-agent framework-agnostic analysis...");

        // Stage 0: Framework Detection
        println!("Stage 0: Framework Detection");
        let framework_detector = FrameworkDetector::new(self.gemini_service.clone());
        let framework_detection = framework_detector
            .detect_frameworks_and_libraries(packages, imported_symbols)
            .await?;

        println!("Detected frameworks: {:?}", framework_detection.frameworks);
        println!(
            "Detected data fetchers: {:?}",
            framework_detection.data_fetchers
        );

        // Stage 1: Call Site Extraction
        println!("Stage 1: Call Site Extraction");
        let call_sites = self.extract_all_call_sites(&files).await?;
        println!("Extracted {} call sites", call_sites.len());

        // Stage 2: Call Site Classification using Classify-Then-Dispatch
        println!("Stage 2: Call Site Classification using Classify-Then-Dispatch");
        let orchestrator = CallSiteOrchestrator::new(self.gemini_service.clone());
        let analysis_results = orchestrator
            .analyze_call_sites(&call_sites, &framework_detection)
            .await?;

        println!(
            "Analysis complete - {} total call sites processed",
            analysis_results.triage_stats.total_call_sites
        );
        self.print_analysis_summary(&analysis_results);

        // Stage 3: Mount Graph Construction
        println!("Stage 3: Mount Graph Construction");
        let mount_graph =
            MountGraph::build_from_analysis_results(&analysis_results, imported_symbols);

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
            mount_graph,
            analysis_results,
        })
    }

    /// Extract call sites from all files using framework-agnostic approach
    async fn extract_all_call_sites(
        &self,
        files: &[std::path::PathBuf],
    ) -> Result<Vec<crate::call_site_extractor::CallSite>, Box<dyn std::error::Error>> {
        let handler = Handler::with_tty_emitter(
            ColorConfig::Auto,
            true,
            false,
            Some(self.source_map.clone()),
        );
        let mut extraction_service = CallSiteExtractionService::new();
        let mut extractors = Vec::new();

        println!("DEBUG: Parsing {} files", files.len());
        for file_path in files {
            if let Some(module) = parse_file(file_path, &self.source_map, &handler) {
                let mut extractor =
                    CallSiteExtractor::new(file_path.clone(), self.source_map.clone());
                swc_ecma_visit::VisitWith::visit_with(&module, &mut extractor);
                println!(
                    "DEBUG: File {:?} - extracted {} call sites",
                    file_path,
                    extractor.call_sites.len()
                );
                extractors.push(extractor);
            } else {
                println!("DEBUG: Failed to parse file {:?}", file_path);
            }
        }

        extraction_service.extract_from_visitors(extractors);
        let call_sites = extraction_service.get_call_sites().to_vec();
        println!(
            "DEBUG: Total call sites after aggregation: {}",
            call_sites.len()
        );
        if !call_sites.is_empty() {
            println!("DEBUG: Sample call sites (first 3):");
            for (i, site) in call_sites.iter().take(3).enumerate() {
                println!(
                    "  {}. {}.{}() at {}",
                    i + 1,
                    site.callee_object,
                    site.callee_property,
                    site.location
                );
            }
        }
        Ok(call_sites)
    }

    fn print_analysis_summary(&self, analysis_results: &AnalysisResults) {
        println!("=== ANALYSIS SUMMARY ===");
        println!("Triage Results:");
        println!(
            "  - HTTP Endpoints: {}",
            analysis_results.triage_stats.endpoints_count
        );
        println!(
            "  - Data Fetching Calls: {}",
            analysis_results.triage_stats.data_fetching_count
        );
        println!(
            "  - Middleware: {}",
            analysis_results.triage_stats.middleware_count
        );
        println!(
            "  - Irrelevant: {}",
            analysis_results.triage_stats.irrelevant_count
        );

        println!("Detailed Extractions:");
        println!(
            "  - Endpoints extracted: {}",
            analysis_results.endpoints.len()
        );
        println!(
            "  - API calls extracted: {}",
            analysis_results.data_fetching_calls.len()
        );
        println!(
            "  - Middleware extracted: {}",
            analysis_results.middleware.len()
        );
        println!(
            "  - Mount relationships extracted: {}",
            analysis_results.mount_relationships.len()
        );
    }

    /// Get framework-aware endpoint analysis for comparison with existing system
    #[allow(dead_code)]
    pub fn get_endpoint_analysis(&self, result: &MultiAgentAnalysisResult) -> EndpointAnalysis {
        let mount_graph = &result.mount_graph;

        EndpointAnalysis {
            producers: mount_graph.get_resolved_endpoints().to_vec(),
            consumers: mount_graph.get_data_calls().to_vec(),
            mount_relationships: mount_graph.get_mounts().to_vec(),
        }
    }

    /// Extract type information from agent analysis results for type checking
    pub fn extract_types_from_analysis(
        &self,
        analysis_results: &AnalysisResults,
    ) -> Vec<serde_json::Value> {
        use crate::analyzer::Analyzer;
        let mut type_infos = Vec::new();

        println!("=== EXTRACT TYPES FROM ANALYSIS DEBUG ===");
        println!(
            "Endpoints to process: {}, Data fetching calls to process: {}",
            analysis_results.endpoints.len(),
            analysis_results.data_fetching_calls.len()
        );

        // 1. Extract types from endpoints (Producers)
        let mut endpoints_with_types = 0;
        for endpoint in &analysis_results.endpoints {
            if let (Some(file), Some(pos), Some(type_str)) = (
                &endpoint.response_type_file,
                endpoint.response_type_position,
                &endpoint.response_type_string,
            ) {
                let alias = Analyzer::generate_common_type_alias_name(
                    &endpoint.path,
                    &endpoint.method,
                    false, // is_request_type (false = response)
                    false, // is_consumer (false = producer)
                );

                type_infos.push(serde_json::json!({
                    "filePath": file,
                    "startPosition": pos,
                    "compositeTypeString": type_str,
                    "alias": alias
                }));
                endpoints_with_types += 1;
                println!(
                    "  Endpoint type extracted: {} {} -> {} (file: {}, pos: {})",
                    endpoint.method, endpoint.path, type_str, file, pos
                );
            }
        }
        println!(
            "Endpoints with type info: {}/{}",
            endpoints_with_types,
            analysis_results.endpoints.len()
        );

        // 2. Extract types from data fetching calls (Consumers)
        // Group calls by (url, method) to assign call numbers matching Analyzer logic
        let mut calls_by_endpoint: HashMap<(String, String), u32> = HashMap::new();
        let mut calls_by_location: HashMap<String, u32> = HashMap::new();
        let mut calls_with_types = 0;

        for call in &analysis_results.data_fetching_calls {
            // Check if this call has type info (file, position, and type string)
            if let (Some(file), Some(pos), Some(type_str)) = (
                &call.expected_type_file,
                call.expected_type_position,
                &call.expected_type_string,
            ) {
                // Generate alias based on URL/method if available, otherwise use location
                let alias = if let (Some(url), Some(method)) = (&call.url, &call.method) {
                    // Increment call counter for this endpoint
                    let counter = calls_by_endpoint
                        .entry((url.clone(), method.clone()))
                        .or_insert(0);
                    *counter += 1;

                    Analyzer::generate_unique_call_alias_name(
                        url, method, false,    // is_request_type (false = response)
                        *counter, // call_number
                        true,     // is_consumer (true = consumer)
                    )
                } else {
                    // For response parsing calls without URL (e.g., .json()), use location-based alias
                    let counter = calls_by_location.entry(call.location.clone()).or_insert(0);
                    *counter += 1;

                    // Extract a simple identifier from location (file:line:col -> line_col)
                    let location_parts: Vec<&str> = call.location.split(':').collect();
                    let line = location_parts.get(1).unwrap_or(&"0");
                    let col = location_parts.get(2).unwrap_or(&"0");
                    format!("ResponseParsingConsumerL{}C{}", line, col)
                };

                type_infos.push(serde_json::json!({
                    "filePath": file,
                    "startPosition": pos,
                    "compositeTypeString": type_str,
                    "alias": alias
                }));
                calls_with_types += 1;
                println!(
                    "  Call type extracted: {} -> {} (file: {}, pos: {})",
                    alias, type_str, file, pos
                );
            }
        }
        println!(
            "Calls with type info: {}/{}",
            calls_with_types,
            analysis_results.data_fetching_calls.len()
        );
        println!("Total type_infos extracted: {}", type_infos.len());
        println!("=== END EXTRACT TYPES FROM ANALYSIS DEBUG ===");

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
