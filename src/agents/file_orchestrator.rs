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
        file_analyzer_agent::{
            DataCallResult, EndpointResult, FileAnalysisResult, FileAnalyzerAgent,
        },
        framework_guidance_agent::FrameworkGuidance,
    },
    cloud_storage::{ManifestRole, ManifestTypeKind},
    config::Config,
    framework_detector::DetectionResult,
    mount_graph::{DataFetchingCall, GraphNode, MountEdge, MountGraph, NodeType, ResolvedEndpoint},
    packages::Packages,
    parser::parse_file,
    services::type_sidecar::{
        InferKind, InferRequestItem, SymbolRequest, TypeResolutionResult, TypeSidecar,
    },
    swc_scanner::{CandidateTarget, SwcScanner},
    type_manifest::{
        build_call_site_id, build_manifest_type_alias, build_manifest_type_alias_with_call_id,
        is_http_method, normalize_manifest_method, parse_file_location,
    },
    url_normalizer::UrlNormalizer,
    visitor::{ImportSymbolExtractor, ImportedSymbol, SymbolKind, TypeSymbolExtractor},
    wrapper_registry::wrapper_rules_for_packages,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;

/// Complete result of file-centric analysis
#[derive(Debug)]
pub struct FileCentricAnalysisResult {
    /// Per-file analysis results
    pub file_results: HashMap<String, FileAnalysisResult>,
    /// Aggregated mount graph
    pub mount_graph: MountGraph,
    /// Processing statistics
    pub stats: ProcessingStats,
    /// Bundled type definitions (if sidecar was used)
    pub bundled_types: Option<String>,
    /// Type resolution result from sidecar
    pub type_resolution: Option<TypeResolutionResult>,
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

type EndpointLookup = HashMap<(String, u32), Vec<(String, String)>>;
type DataCallLookup = HashMap<(String, u32), Vec<(String, String, String)>>;

#[derive(Debug, Default)]
struct SymbolTable {
    local_types: HashSet<String>,
    imported_symbols: HashMap<String, ImportedSymbol>,
}

impl SymbolTable {
    fn import_map(&self) -> HashMap<String, String> {
        let mut import_map = HashMap::new();
        for (local_name, symbol) in &self.imported_symbols {
            import_map.insert(local_name.clone(), symbol.source.clone());
        }
        import_map
    }
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
        let cm: Lrc<SourceMap> = Default::default();
        let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

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
            let candidate_contexts: Vec<String> = scan_result
                .candidates
                .iter()
                .map(|c| serde_json::to_string(c).unwrap_or_default())
                .collect();
            let candidate_map: HashMap<String, CandidateTarget> = scan_result
                .candidates
                .iter()
                .map(|candidate| (candidate.candidate_id.clone(), candidate.clone()))
                .collect();

            let symbol_table = Self::extract_symbol_table(file_path, &cm, &handler);
            let import_map = symbol_table.import_map();

            // STEP 4: Call Gemini with Full File + Patterns + Candidate Targets
            match self
                .file_analyzer
                .analyze_file_with_candidates(
                    &path_str,
                    &content,
                    guidance,
                    &candidate_hints,
                    &candidate_contexts,
                    &import_map,
                )
                .await
            {
                Ok(result) => {
                    // Note: Type positions are now resolved by the TypeSidecar (src/sidecar)
                    // using the compiler-based approach instead of position-based extraction.

                    let mut adjusted = result;
                    Self::apply_candidate_map(&mut adjusted, &candidate_map);
                    Self::validate_type_hints(&mut adjusted, &symbol_table);
                    Self::normalize_unusable_types(&mut adjusted);

                    stats.total_mounts += adjusted.mounts.len();
                    stats.total_endpoints += adjusted.endpoints.len();
                    stats.total_data_calls += adjusted.data_calls.len();
                    stats.files_processed += 1;
                    file_results.insert(path_str, adjusted);
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
            bundled_types: None,
            type_resolution: None,
        })
    }

    /// Collect type requests from analysis results for sidecar processing.
    ///
    /// Returns two vectors:
    /// - `SymbolRequest`: For entries WITH explicit type annotations (primary_type_symbol + type_import_source)
    /// - `InferRequestItem`: For entries WITHOUT explicit type annotations (need inference)
    ///
    /// # Arguments
    /// * `file_results` - Analysis results keyed by file path
    /// * `repo_path` - Path to the repository root (used to convert relative paths to absolute)
    /// * `mount_graph` - Resolved mount graph for canonical method/path aliases
    /// * `config` - Config used for URL normalization
    pub fn collect_type_requests(
        &self,
        file_results: &HashMap<String, FileAnalysisResult>,
        repo_path: &str,
        mount_graph: &MountGraph,
        config: &Config,
    ) -> (
        Vec<SymbolRequest>,
        Vec<InferRequestItem>,
        Vec<(String, String)>,
    ) {
        // Convert repo_path to absolute for path resolution
        let repo_root = std::path::Path::new(repo_path);
        let repo_root_absolute = if repo_root.is_absolute() {
            repo_root.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(repo_root))
                .unwrap_or_else(|_| repo_root.to_path_buf())
                .canonicalize()
                .unwrap_or_else(|_| repo_root.to_path_buf())
        };

        let normalizer = UrlNormalizer::new(config);
        let mut explicit_requests: Vec<SymbolRequest> = Vec::new();
        let mut explicit_seen: HashSet<String> = HashSet::new();
        let mut infer_requests: Vec<InferRequestItem> = Vec::new();
        let mut endpoint_lookup: EndpointLookup = HashMap::new();
        let mut data_call_lookup: DataCallLookup = HashMap::new();
        let mut inline_aliases: Vec<(String, String)> = Vec::new();
        let should_infer_request_body = |method: &str| {
            matches!(
                method,
                "POST" | "PUT" | "PATCH" | "DELETE" | "ALL" | "UNKNOWN"
            )
        };
        let mut push_explicit =
            |symbol_name: String, source_file: String, alias: Option<String>| {
                let key = format!(
                    "{}|{}|{}",
                    source_file,
                    symbol_name,
                    alias.as_deref().unwrap_or("")
                );
                if explicit_seen.insert(key) {
                    explicit_requests.push(SymbolRequest {
                        symbol_name,
                        source_file,
                        alias,
                    });
                }
            };
        let mut push_infer = |file_path: &str,
                              line_number: u32,
                              infer_kind: InferKind,
                              alias: String,
                              span_start: Option<u32>,
                              span_end: Option<u32>| {
            let (Some(start), Some(end)) = (span_start, span_end) else {
                return false;
            };
            infer_requests.push(InferRequestItem {
                file_path: file_path.to_string(),
                line_number,
                infer_kind,
                span_start: Some(start),
                span_end: Some(end),
                alias: Some(alias),
            });
            true
        };

        for endpoint in mount_graph.get_resolved_endpoints() {
            let (file_path, line_number) = parse_file_location(&endpoint.file_location);
            let method = normalize_manifest_method(&endpoint.method);
            endpoint_lookup
                .entry((file_path, line_number))
                .or_default()
                .push((method, endpoint.full_path.clone()));
        }

        for data_call in mount_graph.get_data_calls() {
            if !normalizer.is_probable_url(&data_call.target_url) {
                continue;
            }
            let (file_path, line_number) = parse_file_location(&data_call.file_location);
            let Some(method) = Self::normalize_consumer_method(Some(&data_call.method)) else {
                continue;
            };
            let path = normalizer.extract_path(&data_call.target_url);
            let call_id = build_call_site_id(&file_path, line_number, &method, &path);
            data_call_lookup
                .entry((file_path, line_number))
                .or_default()
                .push((method, path, call_id));
        }

        for (file_path, result) in file_results {
            // Convert file_path to absolute path relative to repo root
            let file_path_absolute = Self::to_absolute_path(file_path, &repo_root_absolute);

            // Process endpoints
            for endpoint in &result.endpoints {
                let line_number = if endpoint.line_number <= 0 {
                    1
                } else {
                    endpoint.line_number as u32
                };
                let lookup_key = (file_path.clone(), line_number);
                let method_fallback = normalize_manifest_method(&endpoint.method);
                let (method, path) = endpoint_lookup
                    .get(&lookup_key)
                    .and_then(|entries| {
                        if entries.len() == 1 {
                            return Some(entries[0].clone());
                        }
                        entries
                            .iter()
                            .find(|(entry_method, entry_path)| {
                                entry_method == &method_fallback
                                    && (entry_path == &endpoint.path
                                        || entry_path.ends_with(&endpoint.path))
                            })
                            .or_else(|| {
                                entries
                                    .iter()
                                    .find(|(entry_method, _)| entry_method == &method_fallback)
                            })
                            .cloned()
                    })
                    .unwrap_or_else(|| (method_fallback.clone(), endpoint.path.clone()));
                let response_alias = build_manifest_type_alias(
                    &method,
                    &path,
                    ManifestRole::Producer,
                    ManifestTypeKind::Response,
                );
                let request_alias = build_manifest_type_alias(
                    &method,
                    &path,
                    ManifestRole::Producer,
                    ManifestTypeKind::Request,
                );

                if let (Some(symbol), Some(import_source)) =
                    (&endpoint.primary_type_symbol, &endpoint.type_import_source)
                {
                    // Explicit type with import source - bundle it
                    push_explicit(
                        symbol.clone(),
                        Self::resolve_import_path(&file_path_absolute, import_source),
                        None,
                    );
                } else if endpoint.primary_type_symbol.is_some()
                    && endpoint.type_import_source.is_none()
                {
                    // Type symbol exists but no import - it might be in the same file
                    if let Some(ref symbol) = endpoint.primary_type_symbol {
                        push_explicit(symbol.clone(), file_path_absolute.clone(), None);
                    }
                }

                let response_inferred = push_infer(
                    &file_path_absolute,
                    line_number,
                    InferKind::ResponseBody,
                    response_alias.clone(),
                    endpoint.response_expression_span_start,
                    endpoint.response_expression_span_end,
                );
                if !response_inferred {
                    if let Some(symbol) = endpoint.primary_type_symbol.as_ref() {
                        inline_aliases.push((response_alias.clone(), symbol.clone()));
                    }
                }

                if should_infer_request_body(&method) {
                    push_infer(
                        &file_path_absolute,
                        line_number,
                        InferKind::RequestBody,
                        request_alias.clone(),
                        endpoint.payload_expression_span_start,
                        endpoint.payload_expression_span_end,
                    );
                }
            }

            // Process data calls
            for data_call in &result.data_calls {
                let line_number = if data_call.line_number <= 0 {
                    1
                } else {
                    data_call.line_number as u32
                };
                if !normalizer.is_probable_url(&data_call.target) {
                    continue;
                }
                let lookup_key = (file_path.clone(), line_number);
                let Some(method_fallback) =
                    Self::normalize_consumer_method(data_call.method.as_deref())
                else {
                    continue;
                };
                let target_path = normalizer.extract_path(&data_call.target);
                let (method, path, call_id) = data_call_lookup
                    .get(&lookup_key)
                    .and_then(|entries| {
                        if entries.len() == 1 {
                            return Some(entries[0].clone());
                        }
                        entries
                            .iter()
                            .find(|(entry_method, entry_path, _)| {
                                entry_method == &method_fallback && entry_path == &target_path
                            })
                            .or_else(|| {
                                entries
                                    .iter()
                                    .find(|(entry_method, _, _)| entry_method == &method_fallback)
                            })
                            .cloned()
                    })
                    .unwrap_or_else(|| {
                        (
                            method_fallback.clone(),
                            target_path.clone(),
                            build_call_site_id(
                                file_path,
                                line_number,
                                &method_fallback,
                                &target_path,
                            ),
                        )
                    });
                let response_alias = build_manifest_type_alias_with_call_id(
                    &method,
                    &path,
                    ManifestRole::Consumer,
                    ManifestTypeKind::Response,
                    Some(&call_id),
                );
                let request_alias = build_manifest_type_alias_with_call_id(
                    &method,
                    &path,
                    ManifestRole::Consumer,
                    ManifestTypeKind::Request,
                    Some(&call_id),
                );

                if let (Some(symbol), Some(import_source)) = (
                    &data_call.primary_type_symbol,
                    &data_call.type_import_source,
                ) {
                    // Explicit type with import source - bundle it
                    push_explicit(
                        symbol.clone(),
                        Self::resolve_import_path(&file_path_absolute, import_source),
                        None,
                    );
                } else if data_call.primary_type_symbol.is_some()
                    && data_call.type_import_source.is_none()
                {
                    // Type symbol exists but no import - it might be in the same file
                    if let Some(ref symbol) = data_call.primary_type_symbol {
                        push_explicit(symbol.clone(), file_path_absolute.clone(), None);
                    }
                }

                let call_inferred = push_infer(
                    &file_path_absolute,
                    line_number,
                    InferKind::CallResult,
                    response_alias.clone(),
                    data_call.call_expression_span_start,
                    data_call.call_expression_span_end,
                );
                if !call_inferred {
                    if let Some(symbol) = data_call.primary_type_symbol.as_ref() {
                        inline_aliases.push((response_alias.clone(), symbol.clone()));
                    }
                }

                if should_infer_request_body(&method) {
                    push_infer(
                        &file_path_absolute,
                        line_number,
                        InferKind::RequestBody,
                        request_alias.clone(),
                        data_call.payload_expression_span_start,
                        data_call.payload_expression_span_end,
                    );
                }
            }
        }

        eprintln!(
            "[FileOrchestrator] Collected {} explicit type requests, {} inference requests, {} inline aliases",
            explicit_requests.len(),
            infer_requests.len(),
            inline_aliases.len()
        );

        (explicit_requests, infer_requests, inline_aliases)
    }

    fn extract_symbol_table(
        file_path: &Path,
        cm: &Lrc<SourceMap>,
        handler: &Handler,
    ) -> SymbolTable {
        let Some(module) = parse_file(file_path, cm, handler) else {
            return SymbolTable::default();
        };

        let mut import_extractor = ImportSymbolExtractor::new();
        module.visit_with(&mut import_extractor);

        let mut type_extractor = TypeSymbolExtractor::new();
        module.visit_with(&mut type_extractor);

        SymbolTable {
            local_types: type_extractor.type_symbols,
            imported_symbols: import_extractor.imported_symbols,
        }
    }

    fn validate_type_hints(result: &mut FileAnalysisResult, symbol_table: &SymbolTable) {
        let validate = |primary: &mut Option<String>, source: &mut Option<String>| {
            let Some(symbol) = primary.as_ref() else {
                *source = None;
                return;
            };

            let (root, has_member) = symbol
                .split_once('.')
                .map(|(root, _)| (root, true))
                .unwrap_or((symbol.as_str(), false));

            if symbol_table.local_types.contains(root) {
                if source.is_none() && !has_member {
                    return;
                }
            } else if let Some(imported) = symbol_table.imported_symbols.get(root) {
                let source_matches = source
                    .as_deref()
                    .map(|value| value == imported.source.as_str())
                    .unwrap_or(false);
                let namespace_ok = if imported.kind == SymbolKind::Namespace {
                    has_member
                } else {
                    !has_member
                };
                if source_matches && namespace_ok {
                    return;
                }
            }

            *primary = None;
            *source = None;
        };

        for endpoint in &mut result.endpoints {
            validate(
                &mut endpoint.primary_type_symbol,
                &mut endpoint.type_import_source,
            );
        }

        for data_call in &mut result.data_calls {
            validate(
                &mut data_call.primary_type_symbol,
                &mut data_call.type_import_source,
            );
        }
    }

    /// Normalize unusable type hints from the LLM so we can force inference instead of padding unknowns.
    fn normalize_unusable_types(result: &mut FileAnalysisResult) {
        let scrub_endpoint = |endpoint: &mut EndpointResult| {
            // Drop framework imports to force inference later.
            let bad_source = matches!(endpoint.type_import_source.as_deref(), Some("express"));
            if bad_source {
                endpoint.type_import_source = None;
                endpoint.primary_type_symbol = None;
            }
        };

        let scrub_data_call = |call: &mut DataCallResult| {
            let bad_source = matches!(call.type_import_source.as_deref(), Some("express"));
            if bad_source {
                call.type_import_source = None;
                call.primary_type_symbol = None;
            }
        };

        for endpoint in &mut result.endpoints {
            scrub_endpoint(endpoint);
        }
        for call in &mut result.data_calls {
            scrub_data_call(call);
        }
    }

    fn apply_candidate_map(
        result: &mut FileAnalysisResult,
        candidate_map: &HashMap<String, CandidateTarget>,
    ) {
        result.endpoints = result
            .endpoints
            .drain(..)
            .filter_map(|mut endpoint| {
                let candidate = candidate_map.get(&endpoint.candidate_id)?;
                endpoint.line_number = candidate.line_number as i32;
                endpoint.call_expression_span_start = Some(candidate.span_start);
                endpoint.call_expression_span_end = Some(candidate.span_end);
                Some(endpoint)
            })
            .collect();

        result.data_calls = result
            .data_calls
            .drain(..)
            .filter_map(|mut data_call| {
                let candidate = candidate_map.get(&data_call.candidate_id)?;
                data_call.line_number = candidate.line_number as i32;
                data_call.call_expression_span_start = Some(candidate.span_start);
                data_call.call_expression_span_end = Some(candidate.span_end);
                Some(data_call)
            })
            .collect();
    }

    /// Resolve types using the TypeSidecar.
    ///
    /// This method collects type requests from the analysis results and sends them
    /// to the sidecar for bundling (explicit) and inference (implicit).
    ///
    /// # Arguments
    /// * `sidecar` - The TypeSidecar instance for type resolution
    /// * `file_results` - Analysis results keyed by file path
    /// * `repo_path` - Path to the repository root (used to convert relative paths to absolute)
    /// * `packages` - Dependency metadata used for wrapper rule selection
    /// * `mount_graph` - Resolved mount graph for canonical method/path aliases
    /// * `config` - Config used for URL normalization
    pub fn resolve_types_with_sidecar(
        &self,
        sidecar: &TypeSidecar,
        file_results: &HashMap<String, FileAnalysisResult>,
        repo_path: &str,
        packages: &Packages,
        mount_graph: &MountGraph,
        config: &Config,
    ) -> Result<TypeResolutionResult, Box<dyn std::error::Error>> {
        let (explicit, infer, inline_aliases) =
            self.collect_type_requests(file_results, repo_path, mount_graph, config);

        eprintln!(
            "[FileOrchestrator] Resolving types: {} explicit, {} inferred",
            explicit.len(),
            infer.len()
        );

        let wrappers = wrapper_rules_for_packages(packages);

        let result = sidecar
            .resolve_all_types(&explicit, &infer, &wrappers)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        let result = self.append_inline_aliases(result, inline_aliases);

        // Log results
        eprintln!(
            "[FileOrchestrator] Type resolution complete: {} manifest entries, {} inferred types, {} failures",
            result.explicit_manifest.len(),
            result.inferred_types.len(),
            result.symbol_failures.len()
        );

        if !result.errors.is_empty() {
            eprintln!(
                "[FileOrchestrator] Type resolution warnings: {:?}",
                result.errors
            );
        }

        Ok(result)
    }

    fn append_inline_aliases(
        &self,
        mut result: TypeResolutionResult,
        inline_aliases: Vec<(String, String)>,
    ) -> TypeResolutionResult {
        if inline_aliases.is_empty() {
            return result;
        }

        let mut combined = result.dts_content.take().unwrap_or_default();
        let mut seen = HashSet::new();

        for (alias, type_string) in inline_aliases {
            if !seen.insert(alias.clone()) {
                continue;
            }
            if Self::dts_defines_alias(&combined, &alias) {
                if Self::replace_unknown_alias(&mut combined, &alias, &type_string) {
                    continue;
                }
                continue;
            }
            if !combined.is_empty() && !combined.ends_with('\n') {
                combined.push('\n');
            }
            combined.push_str("export type ");
            combined.push_str(&alias);
            combined.push_str(" = ");
            combined.push_str(type_string.trim().trim_end_matches(';'));
            combined.push_str(";\n");
        }

        if !combined.is_empty() {
            result.dts_content = Some(combined);
        }

        result
    }

    /// Convert a file path to an absolute path.
    ///
    /// If the path is already absolute, returns it as-is.
    /// Otherwise, resolves it relative to the repo root and canonicalizes.
    fn to_absolute_path(file_path: &str, repo_root_absolute: &std::path::Path) -> String {
        use std::path::Path;

        let path = Path::new(file_path);
        if path.is_absolute() {
            return file_path.to_string();
        }

        // Resolve relative to current directory (which should be where cargo run was executed)
        let resolved = std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf());

        // Canonicalize to resolve .. and . components
        resolved
            .canonicalize()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| {
                // If canonicalize fails, try joining with repo root
                repo_root_absolute.join(path).to_string_lossy().to_string()
            })
    }

    /// Resolve an import path relative to a file.
    ///
    /// Converts relative import paths like "./types/user" to absolute paths.
    fn resolve_import_path(current_file: &str, import_source: &str) -> String {
        use std::path::Path;

        // If import source is already absolute or doesn't start with ., return as-is
        if !import_source.starts_with('.') {
            return import_source.to_string();
        }

        // Get the directory of the current file
        let current_path = Path::new(current_file);
        let current_dir = current_path.parent().unwrap_or(Path::new(""));

        // Join with the import source and normalize
        let resolved = current_dir.join(import_source);

        // Add .ts extension if not present
        let resolved_str = resolved.to_string_lossy().to_string();
        let with_extension = if !resolved_str.ends_with(".ts")
            && !resolved_str.ends_with(".tsx")
            && !resolved_str.ends_with(".js")
            && !resolved_str.ends_with(".jsx")
        {
            format!("{}.ts", resolved_str)
        } else {
            resolved_str
        };

        // Canonicalize to resolve .. and . components
        Path::new(&with_extension)
            .canonicalize()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(with_extension)
    }

    fn dts_defines_alias(content: &str, alias: &str) -> bool {
        let escaped = regex::escape(alias);
        let pattern = format!(r"\b(type|interface|class|enum|namespace)\s+{}\b", escaped);
        match regex::Regex::new(&pattern) {
            Ok(re) => re.is_match(content),
            Err(_) => false,
        }
    }

    fn replace_unknown_alias(content: &mut String, alias: &str, type_string: &str) -> bool {
        let escaped = regex::escape(alias);
        let pattern = format!(r"export\s+type\s+{}\s*=\s*unknown\s*;", escaped);
        let Ok(re) = regex::Regex::new(&pattern) else {
            return false;
        };
        if !re.is_match(content) {
            return false;
        }
        let replacement = format!(
            "export type {} = {};",
            alias,
            type_string.trim().trim_end_matches(';')
        );
        *content = re.replace(content, replacement).to_string();
        true
    }

    fn normalize_consumer_method(method: Option<&str>) -> Option<String> {
        let raw = method.unwrap_or("").trim();
        if raw.is_empty() || raw.eq_ignore_ascii_case("unknown") {
            return Some("GET".to_string());
        }
        let normalized = normalize_manifest_method(raw);
        if is_http_method(&normalized) {
            Some(normalized)
        } else {
            None
        }
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
                let Some(method) = Self::normalize_consumer_method(data_call.method.as_deref())
                else {
                    continue;
                };
                graph.data_calls.push(DataFetchingCall {
                    method,
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
                endpoint.full_path = Self::join_paths(prefix, &endpoint.path);
            }
        }
    }

    fn join_paths(prefix: &str, path: &str) -> String {
        let trimmed_prefix = prefix.trim_end_matches('/');
        let trimmed_path = path.trim_start_matches('/');

        if trimmed_prefix.is_empty() {
            if trimmed_path.is_empty() {
                "/".to_string()
            } else {
                format!("/{}", trimmed_path)
            }
        } else if trimmed_path.is_empty() {
            trimmed_prefix.to_string()
        } else {
            format!("{}/{}", trimmed_prefix, trimmed_path)
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
                    candidate_id: "span:100-140".to_string(),
                    line_number: 5,
                    owner_node: "app".to_string(),
                    method: "GET".to_string(),
                    path: "/health".to_string(),
                    handler_name: "healthCheck".to_string(),
                    pattern_matched: ".get(".to_string(),
                    call_expression_span_start: None,
                    call_expression_span_end: None,
                    payload_expression_span_start: None,
                    payload_expression_span_end: None,
                    response_expression_span_start: None,
                    response_expression_span_end: None,
                    primary_type_symbol: None,
                    type_import_source: None,
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
    fn test_join_paths_avoids_double_slashes() {
        assert_eq!(FileOrchestrator::join_paths("/", "/users"), "/users");
        assert_eq!(FileOrchestrator::join_paths("/api", "/users"), "/api/users");
        assert_eq!(
            FileOrchestrator::join_paths("/api/", "/users"),
            "/api/users"
        );
        assert_eq!(FileOrchestrator::join_paths("", "/users"), "/users");
        assert_eq!(FileOrchestrator::join_paths("/api", "/"), "/api");
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
                    candidate_id: "span:200-260".to_string(),
                    line_number: 15,
                    target: "https://api.example.com/data".to_string(),
                    method: Some("POST".to_string()),
                    pattern_matched: "fetch(".to_string(),
                    call_expression_span_start: None,
                    call_expression_span_end: None,
                    payload_expression_span_start: None,
                    payload_expression_span_end: None,
                    primary_type_symbol: None,
                    type_import_source: None,
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
    fn test_collect_type_requests_skips_non_url_data_calls() {
        let agent_service = AgentService::new("mock".to_string());
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();
        file_results.insert(
            "src/service.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![
                    DataCallResult {
                        candidate_id: "span:300-340".to_string(),
                        line_number: 12,
                        target: "ordersResp".to_string(),
                        method: Some("GET".to_string()),
                        pattern_matched: "resp.json()".to_string(),
                        call_expression_span_start: None,
                        call_expression_span_end: None,
                        payload_expression_span_start: None,
                        payload_expression_span_end: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    },
                    DataCallResult {
                        candidate_id: "span:350-400".to_string(),
                        line_number: 15,
                        target: "https://api.example.com/data".to_string(),
                        method: Some("GET".to_string()),
                        pattern_matched: "fetch(".to_string(),
                        call_expression_span_start: Some(350),
                        call_expression_span_end: Some(400),
                        payload_expression_span_start: None,
                        payload_expression_span_end: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    },
                ],
            },
        );

        let graph = orchestrator.build_mount_graph(&file_results);
        let config = Config::default();
        let (_explicit, infer, _inline) =
            orchestrator.collect_type_requests(&file_results, ".", &graph, &config);

        assert_eq!(infer.len(), 1);
    }

    #[test]
    fn test_collect_type_requests_skips_non_http_methods() {
        let agent_service = AgentService::new("mock".to_string());
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();
        file_results.insert(
            "src/service.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![DataCallResult {
                    candidate_id: "span:410-460".to_string(),
                    line_number: 12,
                    target: "https://api.example.com/data".to_string(),
                    method: Some(".json()".to_string()),
                    pattern_matched: "resp.json()".to_string(),
                    call_expression_span_start: None,
                    call_expression_span_end: None,
                    payload_expression_span_start: None,
                    payload_expression_span_end: None,
                    primary_type_symbol: None,
                    type_import_source: None,
                }],
            },
        );

        let graph = orchestrator.build_mount_graph(&file_results);
        let config = Config::default();
        let (explicit, infer, inline) =
            orchestrator.collect_type_requests(&file_results, ".", &graph, &config);

        assert!(explicit.is_empty());
        assert!(infer.is_empty());
        assert!(inline.is_empty());
    }

    #[test]
    fn test_collect_type_requests_assigns_call_ids() {
        let agent_service = AgentService::new("mock".to_string());
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();
        file_results.insert(
            "src/service.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![
                    DataCallResult {
                        candidate_id: "span:470-520".to_string(),
                        line_number: 10,
                        target: "https://api.example.com/orders".to_string(),
                        method: Some("GET".to_string()),
                        pattern_matched: "fetch(".to_string(),
                        call_expression_span_start: Some(470),
                        call_expression_span_end: Some(520),
                        payload_expression_span_start: None,
                        payload_expression_span_end: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    },
                    DataCallResult {
                        candidate_id: "span:530-580".to_string(),
                        line_number: 20,
                        target: "https://api.example.com/orders".to_string(),
                        method: Some("GET".to_string()),
                        pattern_matched: "fetch(".to_string(),
                        call_expression_span_start: Some(530),
                        call_expression_span_end: Some(580),
                        payload_expression_span_start: None,
                        payload_expression_span_end: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    },
                ],
            },
        );

        let graph = orchestrator.build_mount_graph(&file_results);
        let config = Config::default();
        let (_explicit, infer, _inline) =
            orchestrator.collect_type_requests(&file_results, ".", &graph, &config);

        let mut aliases: Vec<String> = infer.into_iter().filter_map(|item| item.alias).collect();
        aliases.sort();

        assert_eq!(aliases.len(), 2);
        assert!(aliases[0].contains("_Call"));
        assert!(aliases[1].contains("_Call"));
        assert_ne!(aliases[0], aliases[1]);
    }

    #[test]
    fn test_validate_type_hints_rejects_invalid_symbols() {
        let mut result = FileAnalysisResult {
            mounts: vec![],
            endpoints: vec![
                EndpointResult {
                    candidate_id: "span:590-650".to_string(),
                    line_number: 10,
                    owner_node: "app".to_string(),
                    method: "GET".to_string(),
                    path: "/users".to_string(),
                    handler_name: "handler".to_string(),
                    pattern_matched: "app.get".to_string(),
                    call_expression_span_start: None,
                    call_expression_span_end: None,
                    payload_expression_span_start: None,
                    payload_expression_span_end: None,
                    response_expression_span_start: None,
                    response_expression_span_end: None,
                    primary_type_symbol: Some("User".to_string()),
                    type_import_source: Some("react".to_string()),
                },
                EndpointResult {
                    candidate_id: "span:700-740".to_string(),
                    line_number: 12,
                    owner_node: "app".to_string(),
                    method: "GET".to_string(),
                    path: "/models".to_string(),
                    handler_name: "handler".to_string(),
                    pattern_matched: "app.get".to_string(),
                    call_expression_span_start: None,
                    call_expression_span_end: None,
                    payload_expression_span_start: None,
                    payload_expression_span_end: None,
                    response_expression_span_start: None,
                    response_expression_span_end: None,
                    primary_type_symbol: Some("Models.User".to_string()),
                    type_import_source: Some("./models".to_string()),
                },
            ],
            data_calls: vec![DataCallResult {
                candidate_id: "span:660-700".to_string(),
                line_number: 12,
                target: "/users".to_string(),
                method: Some("GET".to_string()),
                pattern_matched: "fetch(".to_string(),
                call_expression_span_start: None,
                call_expression_span_end: None,
                payload_expression_span_start: None,
                payload_expression_span_end: None,
                primary_type_symbol: Some("LocalType".to_string()),
                type_import_source: None,
            }],
        };

        let mut imported_symbols = HashMap::new();
        imported_symbols.insert(
            "User".to_string(),
            ImportedSymbol {
                local_name: "User".to_string(),
                imported_name: "User".to_string(),
                source: "./repo-a_types".to_string(),
                kind: SymbolKind::Named,
            },
        );
        imported_symbols.insert(
            "Models".to_string(),
            ImportedSymbol {
                local_name: "Models".to_string(),
                imported_name: "Models".to_string(),
                source: "./models".to_string(),
                kind: SymbolKind::Namespace,
            },
        );

        let symbol_table = SymbolTable {
            local_types: HashSet::from(["LocalType".to_string()]),
            imported_symbols,
        };

        FileOrchestrator::validate_type_hints(&mut result, &symbol_table);

        let invalid_endpoint = &result.endpoints[0];
        assert!(invalid_endpoint.primary_type_symbol.is_none());
        assert!(invalid_endpoint.type_import_source.is_none());

        let namespace_endpoint = &result.endpoints[1];
        assert_eq!(
            namespace_endpoint.primary_type_symbol.as_deref(),
            Some("Models.User")
        );
        assert_eq!(
            namespace_endpoint.type_import_source.as_deref(),
            Some("./models")
        );

        let data_call = &result.data_calls[0];
        assert_eq!(data_call.primary_type_symbol.as_deref(), Some("LocalType"));
        assert!(data_call.type_import_source.is_none());
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
                        candidate_id: "span:710-740".to_string(),
                        line_number: 5,
                        owner_node: "router".to_string(),
                        method: "GET".to_string(),
                        path: "/".to_string(),
                        handler_name: "listUsers".to_string(),
                        pattern_matched: ".get(".to_string(),
                        call_expression_span_start: None,
                        call_expression_span_end: None,
                        payload_expression_span_start: None,
                        payload_expression_span_end: None,
                        response_expression_span_start: None,
                        response_expression_span_end: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    },
                    EndpointResult {
                        candidate_id: "span:750-780".to_string(),
                        line_number: 10,
                        owner_node: "router".to_string(),
                        method: "POST".to_string(),
                        path: "/".to_string(),
                        handler_name: "createUser".to_string(),
                        pattern_matched: ".post(".to_string(),
                        call_expression_span_start: None,
                        call_expression_span_end: None,
                        payload_expression_span_start: None,
                        payload_expression_span_end: None,
                        response_expression_span_start: None,
                        response_expression_span_end: None,
                        primary_type_symbol: None,
                        type_import_source: None,
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
