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
            EmissionStyle, EndpointResult, FileAnalysisResult, FileAnalyzerAgent,
        },
        framework_guidance_agent::ProtocolGuidance,
    },
    cloud_storage::{ManifestRole, ManifestTypeKind},
    config::Config,
    env_alias::{EnvAliasExtractor, EnvAliasMap, resolve_target_env_alias},
    file_based_router::{MethodSource, RoutingConvention, builtin_conventions, derive_route},
    framework_detector::DetectionResult,
    mount_graph::{DataFetchingCall, GraphNode, MountEdge, MountGraph, NodeType, ResolvedEndpoint},
    operation::{OperationKey, Protocol},
    parser::parse_file,
    services::type_sidecar::{
        ExtractionConfig, InferKind, InferRequestItem, SymbolRequest, TypeResolutionResult,
        TypeSidecar,
    },
    swc_scanner::{CandidateTarget, RouteDescriptorEndpoint, SwcScanner},
    type_manifest::{
        build_call_site_id, build_manifest_type_alias, build_manifest_type_alias_with_call_id,
        is_http_method, normalize_manifest_method, parse_file_location,
    },
    url_normalizer::UrlNormalizer,
    visitor::{ImportSymbolExtractor, ImportedSymbol, SymbolKind, TypeSymbolExtractor},
};
use futures::stream::StreamExt;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;
use tracing::{debug, warn};

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
    /// Files excluded because they could not be parsed. Counted separately
    /// from zero-cost skips: a parse failure silently removes the file's
    /// endpoints from the index. A subset of `files_skipped`.
    pub files_parse_failed: usize,
    /// Files whose only candidates belong to protocols without a registered
    /// analyze-file prompt (e.g. raw WebSocket constructors). Skipped instead
    /// of being fed to the HTTP prompt, which couldn't classify them.
    pub files_skipped_unrouted_protocol: usize,
    pub total_mounts: usize,
    pub total_endpoints: usize,
    /// Endpoints derived structurally from file-based routing conventions
    /// (Next.js app router, etc.) rather than from a call-site scan. A subset
    /// of `total_endpoints`.
    pub file_based_endpoints: usize,
    /// Endpoints derived deterministically from route-descriptor data
    /// (`{ method, path, handler }` in a registry array) rather than from the
    /// file-analyzer LLM. A subset of `total_endpoints`. See #234.
    pub route_descriptor_endpoints: usize,
    pub total_data_calls: usize,
    pub errors: Vec<String>,
}

/// Owner assigned to endpoints declared by file location (file-based routing).
/// These routes have no mount chain — their derived path is already absolute —
/// so the owner is a sentinel that matches no mount during path resolution.
const FILE_BASED_ROUTE_OWNER: &str = "__file_based_route__";

/// Sentinel owner for a route-descriptor endpoint whose handler is absent or not
/// a bare identifier. Like `FILE_BASED_ROUTE_OWNER`, it matches no mount during
/// path resolution, so the descriptor's already-absolute path is used as-is.
const ROUTE_DESCRIPTOR_OWNER: &str = "__route_descriptor__";

/// `pattern_matched` tag for endpoints emitted deterministically from
/// route-descriptor data (#234).
const ROUTE_DESCRIPTOR_PATTERN: &str = "route-descriptor";

type EndpointLookup = HashMap<(String, u32), Vec<(String, String)>>;
type DataCallLookup = HashMap<(String, u32), Vec<(String, String, String)>>;

#[derive(Debug, Default)]
struct SymbolTable {
    local_types: HashSet<String>,
    imported_symbols: HashMap<String, ImportedSymbol>,
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
    /// * `framework_detection` - Framework detection results (used for type scrubbing)
    ///
    /// # Returns
    /// A `FileCentricAnalysisResult` containing per-file results and aggregated graph.
    pub async fn analyze_files(
        &self,
        files: &[PathBuf],
        guidance: &ProtocolGuidance,
        framework_detection: &DetectionResult,
        repo_root: &Path,
        graphql_producer_hints: &crate::graphql::GraphqlProducerHints,
    ) -> Result<FileCentricAnalysisResult, Box<dyn std::error::Error>> {
        debug!("=== AST-GATED FILE-CENTRIC ORCHESTRATOR ===");
        debug!("Processing {} files with SWC gatekeeper", files.len());

        // Per-protocol prompt routing: each protocol with a registered LLM
        // pass analyzes only its own candidates, so prompts stay focused.
        // HTTP is the only routed protocol today.
        let guidance = guidance
            .get(&Protocol::Http)
            .ok_or("missing HTTP guidance: guidance map must contain the http protocol")?;

        let mut file_results: HashMap<String, FileAnalysisResult> = HashMap::new();
        let mut stats = ProcessingStats::default();
        let cm: Lrc<SourceMap> = Default::default();
        let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

        // Routing conventions for file-based routes (Next.js app/pages router,
        // etc.). Empty when no convention-bearing framework is detected, in
        // which case the file-based pass below is a no-op.
        let conventions = builtin_conventions(&framework_detection.frameworks);

        // A file that passed the SWC gatekeeper and is ready for the (expensive) LLM call.
        // The CPU-bound preprocessing (read, scan, symbol table) is done serially up front;
        // the LLM calls themselves are then dispatched concurrently.
        struct PendingFile {
            path_str: String,
            content: String,
            candidate_hints: Vec<String>,
            candidate_contexts: Vec<String>,
            candidate_map: HashMap<String, CandidateTarget>,
            symbol_table: SymbolTable,
            /// `local const -> process.env name` bindings (e.g.
            /// `ORDERS_BASE -> ORDERS_SERVICE_URL`). Used after the LLM pass to
            /// rewrite call targets that interpolate an env-var base URL aliased
            /// through a local const, so the real env-var name reaches
            /// classification and cross-repo matching. See `crate::env_alias`.
            env_alias_map: EnvAliasMap,
            /// Endpoints derived from file-based routing conventions, merged in
            /// after the LLM pass. Empty for non-route files.
            route_endpoints: Vec<EndpointResult>,
            /// Endpoints derived deterministically from route-descriptor data
            /// (`{ method, path, handler }`), merged in after the LLM pass. The
            /// LLM ignores route-as-data, so these are the authoritative source
            /// for such endpoints. Empty for files with no route descriptors.
            descriptor_endpoints: Vec<EndpointResult>,
            /// Repo-global GraphQL producer hint lines (Stage B2), injected into
            /// the user message so the model can link resolver functions in this
            /// file to schema fields. Identical for every file; cloned per-pending
            /// so the concurrent dispatch closure owns its copy. Empty for repos
            /// with no SDL producers.
            graphql_producer_hints: Vec<String>,
        }

        // PHASE 1 (serial, CPU-bound): run the SWC gatekeeper on every file and build the
        // work list of files that actually need an LLM call. Zero-cost skips are recorded here.
        let mut pending: Vec<PendingFile> = Vec::new();
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
                debug!("Skipping empty file: {}", path_str);
                stats.files_skipped += 1;
                continue;
            }

            // STEP 1: Run SWC Scanner (Gatekeeper). Pass the LLM-detected
            // data-fetching packages so import-based recall uses detection's
            // decision rather than a hardcoded package list.
            let scan_result = self.swc_scanner.scan_content(
                file_path,
                &content,
                &framework_detection.data_fetchers,
            );

            // A parse failure excludes the whole file (and any endpoints in
            // it) from the index — surface it instead of letting it look like
            // a healthy file with no API patterns.
            if scan_result.parse_failed {
                warn!(
                    "Failed to parse {} — file excluded from analysis; any endpoints in it \
                     will be missing from the index",
                    path_str
                );
                stats.errors.push(format!("Parse failure: {}", path_str));
                stats.files_skipped += 1;
                stats.files_parse_failed += 1;
                // Store empty result so incremental cache knows this file was processed
                file_results.insert(path_str, FileAnalysisResult::default());
                continue;
            }

            // Protocol dispatch: the HTTP analyze-file prompt only ever sees
            // HTTP candidates. Candidates of protocols without a registered
            // prompt (raw WebSocket/EventSource constructors today) are set
            // aside; a file with only those is skipped, not sent to a prompt
            // that has no instructions for them.
            let (http_candidates, unrouted_candidates): (Vec<_>, Vec<_>) = scan_result
                .candidates
                .into_iter()
                .partition(|candidate| candidate.protocol == Protocol::Http);

            // File-based routing: routes declared by file location (Next.js app
            // router, etc.) have no call-site candidate — the endpoint *is* the
            // exported handler declaration. The path comes from the layout and the
            // methods from exported handler names; both are invisible to a
            // call-site scan, so they are derived deterministically here.
            let route_endpoints = if conventions.is_empty() {
                Vec::new()
            } else {
                let rel_path = file_path.strip_prefix(repo_root).unwrap_or(file_path);
                Self::file_based_endpoints(
                    &self.swc_scanner,
                    rel_path,
                    file_path,
                    &content,
                    &conventions,
                )
            };

            // Route-descriptor routes: a route declared as data
            // (`{ method, path, handler }` in a registry array) is fully
            // structural — method, path, and handler owner are literals the
            // file-analyzer prompt ignores (it only matches framework-call
            // patterns). Emit it deterministically (#234) instead of relying on
            // the LLM. The recall-boost candidate the scanner also raised for
            // such an object is redundant once the route is owned here, so it is
            // dropped from `http_candidates` below (matched by span): a file whose
            // only candidates are deterministically-owned descriptors then skips
            // the LLM entirely, like a file-based route.
            let descriptor_endpoints =
                Self::route_descriptor_endpoints(&self.swc_scanner, file_path, &content);
            let descriptor_spans: HashSet<(u32, u32)> = descriptor_endpoints
                .iter()
                .filter_map(|e| Some((e.call_expression_span_start?, e.call_expression_span_end?)))
                .collect();
            let http_candidates: Vec<_> = http_candidates
                .into_iter()
                .filter(|c| !descriptor_spans.contains(&(c.span_start, c.span_end)))
                .collect();

            // GraphQL resolver routing (Stage B2): a resolver file is loose
            // exported functions with no HTTP route candidate, so the
            // candidate-less skip below would drop it before the file-analyzer
            // ever sees it — and without the SDL producer context it couldn't
            // link a resolver to its field anyway. Rescue it from the skip when
            // ALL of: this repo has SDL producers, the file is co-located with
            // the schema (under an SDL scan root), and it has at least one
            // exported binding (resolver-shaped). Scoped this tightly so only
            // schema-adjacent resolver files reach the LLM, not every
            // exported-function file in the repo. The injected GRAPHQL SCHEMA
            // PRODUCERS section gives the model the field list to link against.
            let is_graphql_resolver_file = !graphql_producer_hints.is_empty()
                && graphql_producer_hints.file_within_scan_roots(file_path)
                // Cheap `export` substring pre-check before the expensive
                // `exported_handlers` SWC reparse: `scan_content` (Step 1) already
                // parsed this file, and a resolver file must contain at least one
                // `export`. `&&` short-circuits, so the reparse runs only when the
                // keyword is present — the common no-exports file avoids a second
                // parse entirely. (Reusing the Step-1 parse directly would mean
                // threading exported-handler data through `ScanResult`, which it
                // does not currently carry; the substring guard is the cheap win.)
                && content.contains("export")
                && !self
                    .swc_scanner
                    .exported_handlers(file_path, &content)
                    .is_empty();

            // STEP 2: Check Relevance - if there are no candidates for a routed
            // protocol, SKIP the (expensive) LLM call. File-based route and
            // route-descriptor endpoints are still recorded: they're derived
            // structurally and need no LLM.
            if http_candidates.is_empty() {
                let structural_endpoints: Vec<EndpointResult> = route_endpoints
                    .iter()
                    .cloned()
                    .chain(descriptor_endpoints.iter().cloned())
                    .collect();
                if !structural_endpoints.is_empty() {
                    debug!(
                        "Structural route(s) (no call-site candidates): {} [{} file-based, {} route-descriptor]",
                        path_str,
                        route_endpoints.len(),
                        descriptor_endpoints.len()
                    );
                    stats.total_endpoints += structural_endpoints.len();
                    stats.file_based_endpoints += route_endpoints.len();
                    stats.route_descriptor_endpoints += descriptor_endpoints.len();
                    file_results.insert(
                        path_str,
                        FileAnalysisResult {
                            endpoints: structural_endpoints,
                            ..Default::default()
                        },
                    );
                    continue;
                } else if is_graphql_resolver_file {
                    // Fall through to the LLM pass with empty HTTP candidates:
                    // the file-analyzer reads the producer-field context and the
                    // file content to emit `graphql_operations`.
                    debug!(
                        "Routed GraphQL resolver file (no HTTP candidates): {}",
                        path_str
                    );
                } else if unrouted_candidates.is_empty() {
                    debug!("Skipped (no API patterns): {} [0 candidates]", path_str);
                    stats.files_skipped += 1;
                    stats.files_skipped_no_candidates += 1;
                    // Store empty result so incremental cache knows this file was processed
                    file_results.insert(path_str, FileAnalysisResult::default());
                    continue;
                } else {
                    debug!(
                        "Skipped (only unrouted-protocol candidates): {} [{} candidate(s)]",
                        path_str,
                        unrouted_candidates.len()
                    );
                    stats.files_skipped += 1;
                    stats.files_skipped_unrouted_protocol += 1;
                    file_results.insert(path_str, FileAnalysisResult::default());
                    continue;
                }
            }

            debug!(
                "Analyzing: {} [{} HTTP candidate(s), {} unrouted]",
                path_str,
                http_candidates.len(),
                unrouted_candidates.len()
            );

            // STEP 3: Prepare Candidate Targets as hints for the LLM
            let candidate_hints: Vec<String> =
                http_candidates.iter().map(|c| c.format_hint()).collect();
            let candidate_contexts: Vec<String> = http_candidates
                .iter()
                .map(|c| serde_json::to_string(c).unwrap_or_default())
                .collect();
            let candidate_map: HashMap<String, CandidateTarget> = http_candidates
                .iter()
                .map(|candidate| (candidate.candidate_id.clone(), candidate.clone()))
                .collect();

            let (symbol_table, env_alias_map) =
                Self::extract_symbol_table(file_path, &cm, &handler);

            pending.push(PendingFile {
                path_str,
                content,
                candidate_hints,
                candidate_contexts,
                candidate_map,
                symbol_table,
                env_alias_map,
                route_endpoints,
                descriptor_endpoints,
                graphql_producer_hints: graphql_producer_hints.lines.clone(),
            });
        }

        // PHASE 2 (concurrent, I/O-bound): dispatch the LLM calls. `AgentService` owns a
        // semaphore (CARRICK_CONCURRENCY_LIMIT, default 20) that enforces the real rate cap,
        // so we eagerly buffer up to that many in-flight requests. Completion order does not
        // affect the result: stats are counts and `file_results` is a map, so the aggregate
        // is deterministic regardless of which call finishes first.
        let concurrency = std::env::var("CARRICK_CONCURRENCY_LIMIT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(20)
            .max(1);

        // STEP 4: Call the file analyzer with Full File + Patterns + Candidate Targets +
        // richer AST-derived import table (Move 3, §9.3 of framework-coverage.md).
        let analyzed: Vec<(PendingFile, Result<FileAnalysisResult, String>)> =
            futures::stream::iter(pending.into_iter().map(|pf| async move {
                let result = self
                    .file_analyzer
                    .analyze_file_with_candidates(
                        &pf.path_str,
                        &pf.content,
                        guidance,
                        &pf.candidate_hints,
                        &pf.candidate_contexts,
                        &pf.symbol_table.imported_symbols,
                        &pf.graphql_producer_hints,
                    )
                    .await
                    .map_err(|e| e.to_string());
                (pf, result)
            }))
            .buffer_unordered(concurrency)
            .collect()
            .await;

        // PHASE 3 (serial): fold the per-file results into the aggregate.
        for (pf, result) in analyzed {
            match result {
                Ok(result) => {
                    // Note: Type positions are now resolved by the TypeSidecar (src/sidecar)
                    // using the compiler-based approach instead of position-based extraction.

                    let mut adjusted = result;
                    Self::apply_candidate_map(&mut adjusted, &pf.candidate_map, &pf.path_str);
                    Self::resolve_env_var_aliases(&mut adjusted, &pf.env_alias_map);
                    Self::validate_type_hints(&mut adjusted, &pf.symbol_table);
                    Self::normalize_unusable_types(&mut adjusted, &framework_detection.frameworks);

                    // Canonicalize LLM-emitted endpoint paths to colon-style params
                    // (`/w/[slug]` -> `/w/:slug`) so they dedupe against the file-based
                    // router's structural entries instead of both surviving and flipping
                    // form between non-deterministic scans.
                    Self::canonicalize_endpoint_paths(&mut adjusted);

                    // Merge file-based route endpoints the LLM pass didn't already
                    // produce. The structural (method, path) facts are authoritative.
                    stats.file_based_endpoints +=
                        Self::merge_file_based_endpoints(&mut adjusted, pf.route_endpoints);

                    // Merge route-descriptor endpoints (`{ method, path, handler }`
                    // data) the LLM pass didn't produce — it ignores route-as-data,
                    // so these are emitted deterministically and are authoritative
                    // for such routes (#234).
                    stats.route_descriptor_endpoints +=
                        Self::merge_file_based_endpoints(&mut adjusted, pf.descriptor_endpoints);

                    stats.total_mounts += adjusted.mounts.len();
                    stats.total_endpoints += adjusted.endpoints.len();
                    stats.total_data_calls += adjusted.data_calls.len();
                    stats.files_processed += 1;
                    file_results.insert(pf.path_str, adjusted);
                }
                Err(e) => {
                    stats
                        .errors
                        .push(format!("Failed to analyze {}: {}", pf.path_str, e));
                    stats.files_skipped += 1;
                }
            }
        }

        debug!("\n=== FILE PROCESSING COMPLETE ===");
        debug!("  - Files processed (LLM calls): {}", stats.files_processed);
        debug!("  - Files skipped (total): {}", stats.files_skipped);
        debug!(
            "  - Zero-cost skips (no API patterns): {}",
            stats.files_skipped_no_candidates
        );
        if stats.files_parse_failed > 0 {
            warn!(
                "{} file(s) failed to parse and are excluded from the index",
                stats.files_parse_failed
            );
        }
        debug!("  - Total mounts: {}", stats.total_mounts);
        debug!("  - Total endpoints: {}", stats.total_endpoints);
        debug!(
            "  - File-based route endpoints: {}",
            stats.file_based_endpoints
        );
        debug!(
            "  - Route-descriptor endpoints: {}",
            stats.route_descriptor_endpoints
        );
        debug!("  - Total data calls: {}", stats.total_data_calls);

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
        /// Locator for type inference: either SWC byte-offset spans or Gemini expression text + line
        enum InferLocator<'a> {
            Span {
                span_start: Option<u32>,
                span_end: Option<u32>,
            },
            Text {
                expression_text: Option<&'a str>,
                expression_line: Option<i32>,
            },
            /// Locate purely by line number (no span, no text). Used for
            /// file-based route handlers, where the only reliable anchor is the
            /// handler's declaration line and the sidecar resolves the function
            /// via `findFunctionByLine`.
            Line,
        }

        let mut push_infer = |file_path: &str,
                              line_number: u32,
                              infer_kind: InferKind,
                              alias: String,
                              locator: InferLocator<'_>| {
            match locator {
                InferLocator::Span {
                    span_start,
                    span_end,
                } => {
                    let (Some(start), Some(end)) = (span_start, span_end) else {
                        return false;
                    };
                    infer_requests.push(InferRequestItem {
                        file_path: file_path.to_string(),
                        line_number,
                        infer_kind,
                        span_start: Some(start),
                        span_end: Some(end),
                        expression_text: None,
                        expression_line: None,
                        alias: Some(alias),
                        param_name: None,
                    });
                    true
                }
                InferLocator::Text {
                    expression_text,
                    expression_line,
                } => {
                    let Some(text) = expression_text else {
                        return false;
                    };
                    if text.is_empty() {
                        return false;
                    }
                    infer_requests.push(InferRequestItem {
                        file_path: file_path.to_string(),
                        line_number,
                        infer_kind,
                        span_start: None,
                        span_end: None,
                        expression_text: Some(text.to_string()),
                        expression_line: expression_line
                            .map(|l| if l > 0 { l as u32 } else { line_number }),
                        alias: Some(alias),
                        param_name: None,
                    });
                    true
                }
                InferLocator::Line => {
                    infer_requests.push(InferRequestItem {
                        file_path: file_path.to_string(),
                        line_number,
                        infer_kind,
                        span_start: None,
                        span_end: None,
                        expression_text: None,
                        expression_line: None,
                        alias: Some(alias),
                        param_name: None,
                    });
                    true
                }
            }
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
            let call_id = build_call_site_id(
                &file_path,
                line_number,
                &OperationKey::http(&method, path.clone()),
            );
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
                if !is_http_method(&method) || !path.starts_with('/') {
                    continue;
                }
                let key = OperationKey::http(&method, path.clone());
                let response_alias = build_manifest_type_alias(
                    &key,
                    ManifestRole::Producer,
                    ManifestTypeKind::Response,
                );
                let request_alias = build_manifest_type_alias(
                    &key,
                    ManifestRole::Producer,
                    ManifestTypeKind::Request,
                );

                // no-payload endpoints have no recoverable response contract:
                // skip the explicit-symbol bundling as well as inference below,
                // so the manifest entry stays honestly `unknown` (with its
                // evidence) instead of publishing a phantom contract from a
                // type hint the handler never sends.
                let no_payload = endpoint.emission_style == Some(EmissionStyle::NoPayload);

                if !no_payload {
                    if let (Some(symbol), Some(import_source)) =
                        (&endpoint.primary_type_symbol, &endpoint.type_import_source)
                    {
                        // Explicit type with import source - bundle it
                        push_explicit(
                            symbol.clone(),
                            Self::resolve_import_path(&file_path_absolute, import_source),
                            Some(response_alias.clone()),
                        );
                    } else if endpoint.primary_type_symbol.is_some()
                        && endpoint.type_import_source.is_none()
                    {
                        // Type symbol exists but no import - it might be in the same file
                        if let Some(ref symbol) = endpoint.primary_type_symbol {
                            push_explicit(
                                symbol.clone(),
                                file_path_absolute.clone(),
                                Some(response_alias.clone()),
                            );
                        }
                    } else if endpoint.type_import_source.is_some()
                        && endpoint.primary_type_symbol.is_none()
                    {
                        warn!(
                            "[FileOrchestrator] Endpoint at {}:{} has import source {:?} but no symbol; relying on inference",
                            file_path, line_number, endpoint.type_import_source
                        );
                    }
                }

                // File-based routes (Next.js app router, etc.) have no call-site
                // payload expression: the handler's return type *is* the response
                // contract (e.g., `export async function GET(): Promise<Response>` or `Promise<NextResponse<User[]>>`, or an
                // inferred `return new Response(...)`). Their stored span points at
                // the whole handler declaration, which the response-body locators
                // would misread as the payload — so request a `FunctionReturn`
                // anchored on the handler line instead, which the sidecar resolves
                // via `findFunctionByLine` and Promise-unwraps. Request-body
                // inference is skipped: a Next.js request body isn't recoverable
                // from the signature.
                if endpoint.owner_node == FILE_BASED_ROUTE_OWNER {
                    // Structurally derived endpoints never carry an
                    // emission_style today, but the no-payload gate must hold
                    // here too if that ever changes — a no-payload claim means
                    // the manifest stays honestly unknown, with no inference.
                    if !no_payload {
                        // The Line locator is infallible, so no inline-alias
                        // fallback is needed here.
                        push_infer(
                            &file_path_absolute,
                            line_number,
                            InferKind::FunctionReturn,
                            response_alias.clone(),
                            InferLocator::Line,
                        );
                    }
                    continue;
                }

                // Route response inference by the model's emission_style
                // classification. `None` (field omitted — e.g. cached
                // pre-emission-style analysis) falls back to imperative-send,
                // which is the historical behavior.
                match endpoint.emission_style {
                    // The handler's return value IS the payload: ask for the
                    // handler's return type. Prefer the text locator — the
                    // sidecar resolves the expression's *containing* function,
                    // which finds the exact handler even when it's a named
                    // reference declared far from the registration line. Fall
                    // back to the registration line (correct for inline
                    // handlers, whose function starts on that line) when the
                    // model gave no expression.
                    Some(EmissionStyle::ReturnValue) => {
                        let _ = push_infer(
                            &file_path_absolute,
                            line_number,
                            InferKind::FunctionReturn,
                            response_alias.clone(),
                            InferLocator::Text {
                                expression_text: endpoint.response_expression_text.as_deref(),
                                expression_line: endpoint.response_expression_line,
                            },
                        ) || push_infer(
                            &file_path_absolute,
                            line_number,
                            InferKind::FunctionReturn,
                            response_alias.clone(),
                            InferLocator::Line,
                        );
                    }
                    // No recoverable payload expression (zero-arg sends,
                    // streams, helper-written payloads): skip inference. The
                    // manifest entry keeps `unknown` with its evidence —
                    // honest, instead of inferring from the wrong node.
                    Some(EmissionStyle::NoPayload) => {}
                    Some(EmissionStyle::ImperativeSend) | None => {
                        let response_inferred = push_infer(
                            &file_path_absolute,
                            line_number,
                            InferKind::ResponseBody,
                            response_alias.clone(),
                            InferLocator::Text {
                                expression_text: endpoint.response_expression_text.as_deref(),
                                expression_line: endpoint.response_expression_line,
                            },
                        ) || push_infer(
                            &file_path_absolute,
                            line_number,
                            InferKind::ResponseBody,
                            response_alias.clone(),
                            InferLocator::Span {
                                span_start: endpoint.call_expression_span_start,
                                span_end: endpoint.call_expression_span_end,
                            },
                        );
                        if !response_inferred
                            && let Some(symbol) = endpoint.primary_type_symbol.as_ref()
                        {
                            inline_aliases.push((response_alias.clone(), symbol.clone()));
                        }
                    }
                }

                if should_infer_request_body(&method) {
                    let _ = push_infer(
                        &file_path_absolute,
                        line_number,
                        InferKind::RequestBody,
                        request_alias.clone(),
                        InferLocator::Text {
                            expression_text: endpoint.payload_expression_text.as_deref(),
                            expression_line: endpoint.payload_expression_line,
                        },
                    ) || push_infer(
                        &file_path_absolute,
                        line_number,
                        InferKind::RequestBody,
                        request_alias.clone(),
                        InferLocator::Span {
                            span_start: endpoint.call_expression_span_start,
                            span_end: endpoint.call_expression_span_end,
                        },
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
                                &OperationKey::http(&method_fallback, target_path.clone()),
                            ),
                        )
                    });
                let key = OperationKey::http(&method, path.clone());
                let response_alias = build_manifest_type_alias_with_call_id(
                    &key,
                    ManifestRole::Consumer,
                    ManifestTypeKind::Response,
                    Some(&call_id),
                );
                let request_alias = build_manifest_type_alias_with_call_id(
                    &key,
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
                        Some(response_alias.clone()),
                    );
                } else if data_call.primary_type_symbol.is_some()
                    && data_call.type_import_source.is_none()
                {
                    // Type symbol exists but no import - it might be in the same file
                    if let Some(ref symbol) = data_call.primary_type_symbol {
                        push_explicit(
                            symbol.clone(),
                            file_path_absolute.clone(),
                            Some(response_alias.clone()),
                        );
                    }
                } else if data_call.type_import_source.is_some()
                    && data_call.primary_type_symbol.is_none()
                {
                    warn!(
                        "[FileOrchestrator] Data call at {}:{} has import source {:?} but no symbol; relying on inference",
                        file_path, line_number, data_call.type_import_source
                    );
                }

                let call_inferred = push_infer(
                    &file_path_absolute,
                    line_number,
                    InferKind::CallResult,
                    response_alias.clone(),
                    InferLocator::Text {
                        expression_text: data_call.call_expression_text.as_deref(),
                        expression_line: data_call.call_expression_line,
                    },
                ) || push_infer(
                    &file_path_absolute,
                    line_number,
                    InferKind::CallResult,
                    response_alias.clone(),
                    InferLocator::Span {
                        span_start: data_call.call_expression_span_start,
                        span_end: data_call.call_expression_span_end,
                    },
                );
                if !call_inferred && let Some(symbol) = data_call.primary_type_symbol.as_ref() {
                    inline_aliases.push((response_alias.clone(), symbol.clone()));
                }

                if should_infer_request_body(&method) {
                    push_infer(
                        &file_path_absolute,
                        line_number,
                        InferKind::RequestBody,
                        request_alias.clone(),
                        InferLocator::Text {
                            expression_text: data_call.payload_expression_text.as_deref(),
                            expression_line: data_call.payload_expression_line,
                        },
                    );
                }
            }
        }

        debug!(
            "[FileOrchestrator] Collected {} explicit type requests, {} inference requests, {} inline aliases",
            explicit_requests.len(),
            infer_requests.len(),
            inline_aliases.len()
        );

        (explicit_requests, infer_requests, inline_aliases)
    }

    /// Build `SymbolRequest`s for Socket.IO payload anchors (#245 Phase 1).
    ///
    /// Sibling to `collect_type_requests`: it routes the deterministically
    /// captured socket payload type through the *same* sidecar bundle path the
    /// HTTP explicit-symbol case uses. Listeners are producers, emitters are
    /// consumers; each resolves to the Response-kind alias.
    ///
    /// The alias MUST be `build_manifest_type_alias(&op.key, role, Response)` —
    /// byte-identical to the alias `append_protocol_manifest_entry` stamped on
    /// the manifest entry — or the resolved `.d.ts` never joins back and the
    /// entry stays `Unknown`. This contract is guarded by a unit test.
    ///
    /// Only ops whose extractor captured a `payload_type_symbol` produce a
    /// request; an absent source means the symbol is declared in the emitting
    /// file, so it is resolved against that file's absolute path.
    pub fn collect_socket_type_requests(
        &self,
        sockets: &crate::socket_io::SocketExtraction,
        repo_path: &str,
    ) -> Vec<SymbolRequest> {
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

        let mut requests: Vec<SymbolRequest> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut push = |op: &crate::socket_io::SocketOp, role: ManifestRole| {
            let Some(symbol) = op.payload_type_symbol.as_ref() else {
                return;
            };
            let file_abs =
                Self::to_absolute_path(&op.file_path.to_string_lossy(), &repo_root_absolute);
            let source_file = match op.payload_type_source.as_ref() {
                Some(import_source) => Self::resolve_import_path(&file_abs, import_source),
                // No import → same-file declaration: resolve against the file.
                None => file_abs,
            };
            let alias = build_manifest_type_alias(&op.key, role, ManifestTypeKind::Response);
            let dedup_key = format!("{}|{}|{}", source_file, symbol, alias);
            if seen.insert(dedup_key) {
                requests.push(SymbolRequest {
                    symbol_name: symbol.clone(),
                    source_file,
                    alias: Some(alias),
                });
            }
        };
        for op in &sockets.listeners {
            push(op, ManifestRole::Producer);
        }
        for op in &sockets.emitters {
            push(op, ManifestRole::Consumer);
        }
        debug!(
            "[FileOrchestrator] Collected {} socket payload type requests",
            requests.len()
        );
        requests
    }

    /// Build `SymbolRequest`s for GraphQL consumer result-type anchors (#248
    /// consumer side).
    ///
    /// Near-copy of `collect_socket_type_requests`: it routes the deterministic
    /// consumer anchor — the TS result type bound at the `client.request<T>(DOC)`
    /// call site (`GraphqlOp::payload_type_symbol`) — through the *same* sidecar
    /// bundle path the HTTP explicit-symbol and socket cases use. Only consumers
    /// carry a `payload_type_symbol` (SDL producers anchor on their SDL type
    /// expression, not a bundled TS symbol), so this is consumer-only.
    ///
    /// The alias MUST be `build_manifest_type_alias(&op.key, Consumer, Response)`
    /// — byte-identical to the alias `add_protocol_manifest_entry` stamped on the
    /// manifest entry in `append_protocol_manifest_entries` — or the resolved
    /// `.d.ts` never joins back and the entry stays `Unknown`. This contract is
    /// guarded by a unit test.
    ///
    /// An absent source means the symbol is declared in the consuming file, so it
    /// is resolved against that file's absolute path (same-file fallback).
    pub fn collect_graphql_type_requests(
        &self,
        graphql: &crate::graphql::GraphqlExtraction,
        repo_path: &str,
    ) -> Vec<SymbolRequest> {
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

        let mut requests: Vec<SymbolRequest> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for op in &graphql.consumers {
            let Some(symbol) = op.payload_type_symbol.as_ref() else {
                continue;
            };
            let file_abs =
                Self::to_absolute_path(&op.file_path.to_string_lossy(), &repo_root_absolute);
            let source_file = match op.payload_type_source.as_ref() {
                Some(import_source) => Self::resolve_import_path(&file_abs, import_source),
                // No import → same-file declaration: resolve against the file.
                None => file_abs,
            };
            let alias = build_manifest_type_alias(
                &op.key,
                ManifestRole::Consumer,
                ManifestTypeKind::Response,
            );
            let dedup_key = format!("{}|{}|{}", source_file, symbol, alias);
            if seen.insert(dedup_key) {
                requests.push(SymbolRequest {
                    symbol_name: symbol.clone(),
                    source_file,
                    alias: Some(alias),
                });
            }
        }
        debug!(
            "[FileOrchestrator] Collected {} graphql consumer type requests",
            requests.len()
        );
        requests
    }

    /// Build `FunctionReturn` infer requests for GraphQL SDL producers whose
    /// resolver location was joined in from the file-analyzer (`graphql_operations`,
    /// Stage B1).
    ///
    /// Producers do NOT use the `SymbolRequest`/bundle path the consumer/socket
    /// anchors use: bundling the SDL anchor symbol (`ApiResponse`) would emit the
    /// still-generic wrapper, not the producer's real response contract. The
    /// producer's contract is the resolver function's RETURN type expanded
    /// (`Promise<ApiResponse<Order>>` → `{ data: …, errors }`), so this points an
    /// `InferKind::FunctionReturn` at the resolver's file/line — exactly the
    /// file-based-route handler path. The sidecar resolves the fn return,
    /// Promise/async-iterator-unwraps it, and structurally expands it.
    ///
    /// The alias MUST be `build_manifest_type_alias(&op.key, Producer, Response)`
    /// — byte-identical to the alias `add_protocol_manifest_entry` stamped on the
    /// producer manifest entry — or the inferred type never joins back and the
    /// entry stays `Unknown`. This is the load-bearing join, guarded by a unit
    /// test exactly as the consumer side is.
    ///
    /// Only producers with BOTH `resolver_file` and `resolver_line` set produce a
    /// request; an SDL producer with no matched LLM op stays inferred-from-nothing
    /// (it keeps its SDL anchor, but no expanded response contract).
    pub fn collect_graphql_producer_infer_requests(
        &self,
        graphql: &crate::graphql::GraphqlExtraction,
        repo_path: &str,
    ) -> Vec<InferRequestItem> {
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

        let mut requests: Vec<InferRequestItem> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for op in &graphql.producers {
            let (Some(resolver_file), Some(resolver_line)) =
                (op.resolver_file.as_ref(), op.resolver_line)
            else {
                continue;
            };
            let file_abs =
                Self::to_absolute_path(&resolver_file.to_string_lossy(), &repo_root_absolute);
            let alias = build_manifest_type_alias(
                &op.key,
                ManifestRole::Producer,
                ManifestTypeKind::Response,
            );
            let dedup_key = format!("{}|{}|{}", file_abs, resolver_line, alias);
            if seen.insert(dedup_key) {
                requests.push(InferRequestItem {
                    file_path: file_abs,
                    line_number: resolver_line,
                    span_start: None,
                    span_end: None,
                    expression_text: None,
                    expression_line: None,
                    infer_kind: InferKind::FunctionReturn,
                    alias: Some(alias),
                    param_name: None,
                });
            }
        }
        debug!(
            "[FileOrchestrator] Collected {} graphql producer infer requests",
            requests.len()
        );
        requests
    }

    /// Parse a file once and extract both the symbol table and the env-var
    /// alias map (`local const -> process.env name`). Sharing the parse keeps
    /// the per-file CPU cost flat — both passes are cheap AST walks.
    fn extract_symbol_table(
        file_path: &Path,
        cm: &Lrc<SourceMap>,
        handler: &Handler,
    ) -> (SymbolTable, EnvAliasMap) {
        let Some(module) = parse_file(file_path, cm, handler) else {
            return (SymbolTable::default(), EnvAliasMap::default());
        };

        let mut import_extractor = ImportSymbolExtractor::new();
        module.visit_with(&mut import_extractor);

        let mut type_extractor = TypeSymbolExtractor::new();
        module.visit_with(&mut type_extractor);

        let env_alias_map = EnvAliasExtractor::build(&module);

        (
            SymbolTable {
                local_types: type_extractor.type_symbols,
                imported_symbols: import_extractor.imported_symbols,
            },
            env_alias_map,
        )
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
    ///
    /// Checks BOTH `type_import_source` AND `primary_type_symbol` against all detected frameworks.
    /// This prevents the LLM from using framework namespace types (e.g., `express`, `fastify`)
    /// as payload types, which would resolve to the framework's root namespace instead of actual data.
    fn normalize_unusable_types(result: &mut FileAnalysisResult, frameworks: &[String]) {
        let scrub = |primary: &mut Option<String>, source: &mut Option<String>| {
            // Check type_import_source against ALL detected frameworks
            if let Some(src) = source.as_deref()
                && frameworks.iter().any(|f| f == src)
            {
                *primary = None;
                *source = None;
                return;
            }
            // Check primary_type_symbol: if it matches a framework package name
            // (the default import), it's a framework namespace, not a payload type
            if let Some(sym) = primary.as_deref() {
                let sym_lower = sym.to_lowercase();
                if frameworks.iter().any(|f| f.to_lowercase() == sym_lower) {
                    *primary = None;
                    *source = None;
                }
            }
        };

        for endpoint in &mut result.endpoints {
            scrub(
                &mut endpoint.primary_type_symbol,
                &mut endpoint.type_import_source,
            );
        }
        for call in &mut result.data_calls {
            scrub(&mut call.primary_type_symbol, &mut call.type_import_source);
        }
    }

    /// Derive endpoints for a file whose route is declared by its location in
    /// the project layout (file-based routing) rather than by a call expression
    /// the SWC gatekeeper can see. `derive_route` supplies the path from the
    /// filesystem; the exported handler extractor supplies the HTTP methods and
    /// declaration spans. Neither is recoverable from a call-site scan, so these
    /// endpoints are built deterministically.
    ///
    /// Payload/response *symbol* fields are left empty here: the structural
    /// facts (method and path) are owned at synthesis time, while the response
    /// type is recovered downstream in `collect_type_requests`, which asks the
    /// sidecar for the handler's (Promise-unwrapped) return type — the response
    /// contract for a file-based route.
    ///
    /// `pub` + `#[doc(hidden)]`: this is exposed only so the end-to-end fixture
    /// test (`tests/file_based_routing_test.rs`) can drive the real synthesis
    /// path. It is not part of the supported crate API.
    #[doc(hidden)]
    pub fn file_based_endpoints(
        scanner: &SwcScanner,
        rel_path: &Path,
        file_path: &Path,
        content: &str,
        conventions: &[RoutingConvention],
    ) -> Vec<EndpointResult> {
        let Some(route) = derive_route(rel_path, conventions) else {
            return Vec::new();
        };

        match route.method_source {
            // App-router style: one exported function per HTTP method. The export
            // name *is* the method (GET/POST/...), and its declaration span lets
            // the sidecar locate the handler body later.
            MethodSource::ExportName => scanner
                .exported_handlers(file_path, content)
                .into_iter()
                .filter(|h| is_http_method(&h.name))
                .map(|h| {
                    let method = h.name.to_uppercase();
                    EndpointResult {
                        candidate_id: format!("file-route:{}:{}", method, h.span_start),
                        line_number: h.line_number as i32,
                        owner_node: FILE_BASED_ROUTE_OWNER.to_string(),
                        method,
                        path: route.path.clone(),
                        handler_name: h.name.clone(),
                        pattern_matched: route.convention.clone(),
                        call_expression_span_start: Some(h.span_start),
                        call_expression_span_end: Some(h.span_end),
                        payload_expression_text: None,
                        payload_expression_line: None,
                        response_expression_text: None,
                        response_expression_line: None,
                        emission_style: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    }
                })
                .collect(),
            // Pages-router style: a single default export serves every method. The
            // concrete method set isn't recoverable from the layout alone, so we
            // leave these to a follow-up rather than emit an endpoint with an
            // unknown method (which the mount graph would drop anyway).
            MethodSource::DefaultExport => Vec::new(),
        }
    }

    /// Build deterministic endpoints for routes declared as data
    /// (`{ method: 'GET', path: '/health', handler: healthCheckHandler }` in a
    /// registry array). The method, path, and handler owner are all structural
    /// facts the file-analyzer prompt ignores (it only matches framework-call
    /// patterns), so they are emitted directly instead of through the LLM (#234).
    ///
    /// The owner is the handler identifier (`healthCheckHandler`), never the
    /// HTTP-method literal — the owner-fabrication trap (#227). The descriptor
    /// path is already absolute and carries no mount chain, so (like file-based
    /// routes) the owner resolves to no mount prefix and the path is used as-is.
    /// Descriptors with no resolvable handler fall back to a sentinel owner.
    fn route_descriptor_endpoints(
        scanner: &SwcScanner,
        file_path: &Path,
        content: &str,
    ) -> Vec<EndpointResult> {
        scanner
            .route_descriptor_endpoints(file_path, content)
            .into_iter()
            .filter(|d| is_http_method(&d.method))
            .map(|d: RouteDescriptorEndpoint| {
                let method = d.method.to_uppercase();
                let handler = d
                    .handler
                    .unwrap_or_else(|| ROUTE_DESCRIPTOR_OWNER.to_string());
                EndpointResult {
                    candidate_id: format!("route-descriptor:{}:{}", method, d.span_start),
                    line_number: d.line_number as i32,
                    owner_node: handler.clone(),
                    method,
                    path: d.path,
                    handler_name: handler,
                    pattern_matched: ROUTE_DESCRIPTOR_PATTERN.to_string(),
                    call_expression_span_start: Some(d.span_start),
                    call_expression_span_end: Some(d.span_end),
                    payload_expression_text: None,
                    payload_expression_line: None,
                    response_expression_text: None,
                    response_expression_line: None,
                    emission_style: None,
                    primary_type_symbol: None,
                    type_import_source: None,
                }
            })
            .collect()
    }

    /// Append structurally derived endpoints (file-based routes and
    /// route-descriptor data) the LLM pass didn't already produce (matched by
    /// method + path), keeping the deterministic entries. Returns the number
    /// actually added.
    fn merge_file_based_endpoints(
        result: &mut FileAnalysisResult,
        route_endpoints: Vec<EndpointResult>,
    ) -> usize {
        let mut added = 0;
        for ep in route_endpoints {
            let duplicate = result
                .endpoints
                .iter()
                .any(|e| e.method.eq_ignore_ascii_case(&ep.method) && e.path == ep.path);
            if !duplicate {
                result.endpoints.push(ep);
                added += 1;
            }
        }
        added
    }

    /// Canonicalize a route path to colon-style params so structurally identical
    /// endpoints from the LLM pass (`/w/[slug]`) and the file-based router
    /// (`/w/:slug`) dedupe to one entry instead of both surviving and flipping
    /// form between non-deterministic scans. `[id]` -> `:id`, `[...rest]` -> `**`;
    /// `:id`, `*`, and literal segments are left unchanged (idempotent).
    fn canonicalize_route_path(path: &str) -> String {
        let mut out = String::with_capacity(path.len());
        for (i, seg) in path.split('/').enumerate() {
            if i > 0 {
                out.push('/');
            }
            // Catch-all (`[...rest]`) and optional catch-all (`[[...rest]]`) both
            // map to the router's `**`; ordinary dynamic segments (`[id]`, `[[id]]`)
            // map to `:id`.
            let is_catch_all = (seg.starts_with("[...") && seg.ends_with(']'))
                || (seg.starts_with("[[...") && seg.ends_with("]]"));
            if is_catch_all {
                out.push_str("**");
            } else if seg.len() > 2 && seg.starts_with('[') && seg.ends_with(']') {
                // trim() mirrors the router's sanitize_param so whitespace-jittered
                // LLM output (`[ slug ]`) still dedupes against the router's `:slug`.
                let inner = seg.trim_matches(|c| c == '[' || c == ']').replace('.', "");
                out.push(':');
                out.push_str(inner.trim());
            } else {
                out.push_str(seg);
            }
        }
        out
    }

    /// Rewrite every LLM-emitted endpoint path to the canonical colon form before
    /// the file-based merge, so the structural router entries dedupe against them.
    fn canonicalize_endpoint_paths(result: &mut FileAnalysisResult) {
        for ep in &mut result.endpoints {
            let canon = Self::canonicalize_route_path(&ep.path);
            if canon != ep.path {
                ep.path = canon;
            }
        }
    }

    fn apply_candidate_map(
        result: &mut FileAnalysisResult,
        candidate_map: &HashMap<String, CandidateTarget>,
        file_path: &str,
    ) {
        // Endpoints: keep filter_map (endpoints without candidates are unreliable),
        // but a drop means an endpoint the LLM reported vanishes from the index —
        // log which, so silent loss is at least diagnosable.
        let mut dropped_endpoints: Vec<String> = Vec::new();
        result.endpoints = result
            .endpoints
            .drain(..)
            .filter_map(|mut endpoint| {
                let Some(candidate) = candidate_map.get(&endpoint.candidate_id) else {
                    dropped_endpoints.push(format!(
                        "{} {} (candidate_id '{}')",
                        endpoint.method, endpoint.path, endpoint.candidate_id
                    ));
                    return None;
                };
                endpoint.line_number = candidate.line_number as i32;
                endpoint.call_expression_span_start = Some(candidate.span_start);
                endpoint.call_expression_span_end = Some(candidate.span_end);
                Some(endpoint)
            })
            .collect();

        if !dropped_endpoints.is_empty() {
            warn!(
                "[FileOrchestrator] {} endpoint(s) in {} dropped — no matching SWC candidate: {}",
                dropped_endpoints.len(),
                file_path,
                dropped_endpoints.join(", ")
            );
        }

        // Data calls: preserve even without candidate match (inline aliases still work)
        let mut dropped_count = 0;
        result.data_calls = result
            .data_calls
            .drain(..)
            .map(|mut data_call| {
                if let Some(candidate) = candidate_map.get(&data_call.candidate_id) {
                    data_call.line_number = candidate.line_number as i32;
                    data_call.call_expression_span_start = Some(candidate.span_start);
                    data_call.call_expression_span_end = Some(candidate.span_end);
                } else {
                    dropped_count += 1;
                }
                data_call
            })
            .collect();

        if dropped_count > 0 {
            warn!(
                "[FileOrchestrator] {} data call(s) had no matching SWC candidate (spans unavailable)",
                dropped_count
            );
        }
    }

    /// Rewrite data-call targets that interpolate an env-var base URL aliased
    /// through a local const so the real `process.env` name reaches downstream
    /// classification and cross-repo matching (#218).
    ///
    /// The file analyzer emits the target verbatim — e.g.
    /// `${ORDERS_BASE}/orders/${id}` for
    /// `const ORDERS_BASE = process.env.ORDERS_SERVICE_URL ?? "...";`. Without
    /// this pass, `Config::is_internal_call` and the cross-repo matcher key on
    /// the local const `ORDERS_BASE` rather than `ORDERS_SERVICE_URL`, so the
    /// edge never forms. Rewriting the leading `${ALIAS}` to
    /// `${process.env.NAME}` funnels the call back through the existing
    /// direct-`process.env` handling instead of duplicating env-var parsing.
    fn resolve_env_var_aliases(result: &mut FileAnalysisResult, env_alias_map: &EnvAliasMap) {
        if env_alias_map.is_empty() {
            return;
        }
        for data_call in &mut result.data_calls {
            if let Some(resolved) = resolve_target_env_alias(&data_call.target, env_alias_map) {
                data_call.target = resolved;
            }
        }
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
    /// * `extraction_config` - Agent-generated machinery-unwrap rules
    /// * `mount_graph` - Resolved mount graph for canonical method/path aliases
    /// * `config` - Config used for URL normalization
    /// * `extra_explicit` - Deterministically-collected explicit symbol
    ///   requests for non-HTTP protocols (socket payload anchors, #245)
    /// * `extra_infer` - Deterministically-collected `FunctionReturn` infer
    ///   requests for non-HTTP protocols (GraphQL producer resolver returns,
    ///   Stage B1). Unlike `extra_explicit`, these go through the infer path so
    ///   the resolver return is expanded, not bundled as the generic SDL anchor.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_types_with_sidecar(
        &self,
        sidecar: &TypeSidecar,
        file_results: &HashMap<String, FileAnalysisResult>,
        repo_path: &str,
        extraction_config: Option<&ExtractionConfig>,
        mount_graph: &MountGraph,
        config: &Config,
        extra_explicit: &[SymbolRequest],
        extra_infer: &[InferRequestItem],
    ) -> Result<TypeResolutionResult, Box<dyn std::error::Error>> {
        let (mut explicit, mut infer, inline_aliases) =
            self.collect_type_requests(file_results, repo_path, mount_graph, config);

        // Deterministically-collected explicit requests for non-HTTP protocols
        // (today: Socket.IO payload anchors, #245). They use the same
        // `SymbolRequest` shape and bundle path as the HTTP explicit case; the
        // alias each carries matches its manifest entry so the enrich-join lands.
        explicit.extend_from_slice(extra_explicit);

        // Deterministically-collected infer requests for non-HTTP protocols
        // (today: GraphQL producer resolver returns, Stage B1). A producer's real
        // response contract is the resolver's RETURN type expanded, so it takes
        // the `FunctionReturn` infer path (mirroring file-based routes), NOT the
        // bundle path — bundling the SDL anchor would emit the generic wrapper.
        // The alias each carries matches its manifest entry so the join lands.
        infer.extend_from_slice(extra_infer);

        debug!(
            "[FileOrchestrator] Resolving types: {} explicit ({} from non-HTTP protocols), {} inferred ({} from non-HTTP protocols)",
            explicit.len(),
            extra_explicit.len(),
            infer.len(),
            extra_infer.len()
        );

        let result = sidecar
            .resolve_all_types(&explicit, &infer, extraction_config)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        let result = self.append_inline_aliases(result, inline_aliases);

        // Log results
        debug!(
            "[FileOrchestrator] Type resolution complete: {} manifest entries, {} inferred types, {} failures",
            result.explicit_manifest.len(),
            result.inferred_types.len(),
            result.symbol_failures.len()
        );

        // Cap warn-level error emissions the same way per-symbol failures are
        // capped below: a TS-loose codebase can produce dozens of multi-line
        // type-resolution errors that would otherwise dominate the 5 MB log
        // tail and evict the genuinely-novel diagnostics. Spillover stays at
        // debug — visible with --verbose / in the file log.
        const ERROR_WARN_CAP: usize = 10;
        let total_errors = result.errors.len();
        if total_errors > 0 {
            let cap = ERROR_WARN_CAP.min(total_errors);
            warn!(
                "[FileOrchestrator] Type resolution warnings: {:?}",
                &result.errors[..cap]
            );
            if total_errors > cap {
                warn!(
                    shown = cap,
                    suppressed = total_errors - cap,
                    "[FileOrchestrator] Additional type resolution warnings (run with --verbose to see all)"
                );
                debug!(
                    "[FileOrchestrator] Suppressed type resolution warnings: {:?}",
                    &result.errors[cap..]
                );
            }
        }

        // Per-symbol failures carry the actual diagnostic detail (which symbol,
        // which file, why). Cap warn-level emissions so a TS-loose codebase
        // with hundreds of unresolvable types doesn't dominate the 5 MB log
        // tail and evict the actually-novel diagnostic in a failed run.
        // Spillover stays at debug — visible with --verbose or in the file
        // log, but doesn't push noise into uploaded artifacts.
        const SYMBOL_FAILURE_WARN_CAP: usize = 20;
        let total = result.symbol_failures.len();
        let cap = SYMBOL_FAILURE_WARN_CAP.min(total);
        for failure in &result.symbol_failures[..cap] {
            warn!(
                symbol = %failure.symbol_name,
                source_file = %failure.source_file,
                reason = %failure.reason,
                "[FileOrchestrator] Symbol failed to resolve"
            );
        }
        if total > cap {
            warn!(
                shown = cap,
                suppressed = total - cap,
                "[FileOrchestrator] Additional symbol failures (run with --verbose to see all)"
            );
            for failure in &result.symbol_failures[cap..] {
                debug!(
                    symbol = %failure.symbol_name,
                    source_file = %failure.source_file,
                    reason = %failure.reason,
                    "[FileOrchestrator] Symbol failed to resolve"
                );
            }
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
    /// Bare specifiers (e.g. `types/user`) are also resolved against the
    /// nearest `tsconfig.json#compilerOptions.baseUrl` so TypeScript's
    /// classic non-relative resolution works — consistent with `tsc` behaviour
    /// when `baseUrl` is set. If neither relative nor baseUrl resolution
    /// finds a real file, the original specifier is returned unchanged so
    /// node_modules packages like `react` still pass through.
    fn resolve_import_path(current_file: &str, import_source: &str) -> String {
        use std::path::Path;

        let current_dir = Path::new(current_file).parent().unwrap_or(Path::new(""));

        if import_source.starts_with('.') {
            // Relative import — join against the file's own directory.
            let resolved = current_dir.join(import_source);
            let resolved_str = resolved.to_string_lossy().to_string();
            return Self::canonicalize_or_probe(&resolved_str).unwrap_or_else(|| {
                // Nothing matched on disk. Preserve pre-2026-05 behaviour so
                // downstream mount linking still sees a plausible path. If
                // the import already ends in a TS-family extension, return
                // the resolved path as-is (avoid `.ts.ts` double-extension);
                // otherwise append `.ts` as a default.
                if Self::has_ts_extension(&resolved_str) {
                    resolved_str
                } else {
                    let fallback = format!("{}.ts", resolved_str);
                    Path::new(&fallback)
                        .canonicalize()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or(fallback)
                }
            });
        }

        // Bare specifier — only attempt baseUrl resolution if a tsconfig in
        // the file's ancestry sets `compilerOptions.baseUrl` *explicitly*.
        // `tsc` only enables non-relative module resolution against baseUrl
        // when it's set; defaulting to "." here would shadow real
        // node_modules packages. Falling through returns the source
        // unchanged so package imports (`react`, `axios`) still flow through.
        if let Some((tsconfig_dir, base_url)) = Self::find_tsconfig_base_url(current_dir)
            && let Some(found) = Self::canonicalize_or_probe(
                tsconfig_dir
                    .join(&base_url)
                    .join(import_source)
                    .to_string_lossy()
                    .as_ref(),
            )
        {
            return found;
        }

        import_source.to_string()
    }

    /// Returns true if `path` ends in a TypeScript-family source extension.
    fn has_ts_extension(path: &str) -> bool {
        path.ends_with(".ts")
            || path.ends_with(".tsx")
            || path.ends_with(".js")
            || path.ends_with(".jsx")
    }

    /// Probe a path on disk and return a canonicalized absolute string if
    /// it (or one of the standard `.ts/.tsx/.js/.jsx`/`index.*` candidates)
    /// exists. Returns `None` when nothing matches; callers decide on a
    /// fallback. If the input already has a TS-family extension we only
    /// probe that exact path — extension-swapping isn't TS resolver
    /// behaviour and would mask import bugs.
    fn canonicalize_or_probe(base: &str) -> Option<String> {
        use std::path::Path;

        if Self::has_ts_extension(base) {
            return if Path::new(base).exists() {
                Some(
                    Path::new(base)
                        .canonicalize()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| base.to_string()),
                )
            } else {
                None
            };
        }

        let candidates = [
            format!("{}.ts", base),
            format!("{}.tsx", base),
            format!("{}.js", base),
            format!("{}.jsx", base),
            format!("{}/index.ts", base),
            format!("{}/index.tsx", base),
            format!("{}/index.js", base),
            format!("{}/index.jsx", base),
        ];

        for candidate in &candidates {
            if Path::new(candidate).exists() {
                return Some(
                    Path::new(candidate)
                        .canonicalize()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| candidate.clone()),
                );
            }
        }
        None
    }

    /// Walk up from `start_dir` looking for `tsconfig.json`. Return its
    /// directory and the resolved `compilerOptions.baseUrl` only if the
    /// option is *explicitly set* — matches `tsc` behaviour, which only
    /// enables baseUrl-based non-relative resolution when configured.
    /// Returns `None` for tsconfigs that omit baseUrl (or for repos with
    /// no tsconfig at all). Path aliases (`compilerOptions.paths`) and
    /// `extends` inheritance are out of scope here.
    fn find_tsconfig_base_url(start_dir: &std::path::Path) -> Option<(std::path::PathBuf, String)> {
        let mut dir = Some(start_dir);
        while let Some(d) = dir {
            let tsconfig = d.join("tsconfig.json");
            if tsconfig.is_file()
                && let Ok(text) = std::fs::read_to_string(&tsconfig)
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
                && let Some(base_url) = json
                    .get("compilerOptions")
                    .and_then(|c| c.get("baseUrl"))
                    .and_then(|v| v.as_str())
            {
                return Some((d.to_path_buf(), base_url.to_string()));
            }
            dir = d.parent();
        }
        None
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
    pub fn build_mount_graph(
        &self,
        file_results: &HashMap<String, FileAnalysisResult>,
    ) -> MountGraph {
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
                let method = endpoint.method.trim().to_uppercase();
                if !is_http_method(&method) {
                    continue; // Skip non-HTTP methods (e.g., "use", empty)
                }

                // Try to resolve the owner using import information
                let resolved_owner =
                    self.resolve_endpoint_owner(&graph, &endpoint.owner_node, file_path);

                graph.endpoints.push(ResolvedEndpoint {
                    method,
                    path: endpoint.path.clone(),
                    full_path: endpoint.path.clone(), // Will be resolved later
                    handler: Some(endpoint.handler_name.clone()),
                    owner: resolved_owner,
                    file_location: format!("{}:{}", file_path, endpoint.line_number),
                    middleware_chain: Vec::new(),
                    repo_name: None,
                    service_name: None,
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
                // Drop calls whose target is not a real outgoing-call route
                // (SDK ops, bare identifiers, member expressions). Filtering at
                // the producer keeps the uploaded cross-repo index clean, not
                // just the local report.
                if !crate::analyzer::is_valid_route_shape(&data_call.target) {
                    debug!(
                        "Skipping data call with non-route target: {} ({})",
                        data_call.target, file_path
                    );
                    continue;
                }
                graph.data_calls.push(DataFetchingCall {
                    method,
                    target_url: data_call.target.clone(),
                    client: data_call.pattern_matched.clone(),
                    file_location: format!("{}:{}", file_path, data_call.line_number),
                    call_kind: data_call.call_kind,
                    repo_name: None,
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
            // Idempotent guard: if the endpoint path already carries this prefix,
            // don't apply it twice. This happens when a constructor-carried prefix
            // is baked into the endpoint path AND also (redundantly) emitted as the
            // mount's path_prefix — without the guard that doubled to
            // `/api/v1/api/v1/status`. Match on a segment boundary so `/api` does
            // not spuriously swallow `/apixyz`. Framework-agnostic.
            let pfx = if trimmed_prefix.starts_with('/') {
                trimmed_prefix.to_string()
            } else {
                format!("/{}", trimmed_prefix)
            };
            let full = format!("/{}", trimmed_path);
            match full.strip_prefix(&pfx) {
                // Already prefixed (exact, or at a segment boundary) — don't double it.
                Some(rest) if rest.is_empty() || rest.starts_with('/') => full,
                _ => format!("{}/{}", trimmed_prefix, trimmed_path),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::file_analyzer_agent::{DataCallResult, EndpointResult, MountResult};

    #[test]
    fn join_paths_does_not_double_a_baked_prefix() {
        // The double-prefix bug: a constructor-carried prefix baked into the
        // endpoint path AND also emitted as the mount prefix must resolve once.
        assert_eq!(
            FileOrchestrator::join_paths("/api/v1", "/api/v1/status"),
            "/api/v1/status"
        );
        // Exact match (prefix == path) also collapses to one.
        assert_eq!(
            FileOrchestrator::join_paths("/api/v1", "/api/v1"),
            "/api/v1"
        );
        // Normal mount-site prefix (path has no prefix) still applies.
        assert_eq!(
            FileOrchestrator::join_paths("/api/v1", "/status"),
            "/api/v1/status"
        );
        // No false positive: a shared textual prefix that is NOT a segment
        // boundary must still be joined.
        assert_eq!(
            FileOrchestrator::join_paths("/api", "/apixyz"),
            "/api/apixyz"
        );
        // Empty prefix passes the path through.
        assert_eq!(FileOrchestrator::join_paths("", "/users"), "/users");
    }

    /// Regression: `tsconfig.json` with `"baseUrl": "."` makes
    /// `import { X } from "types/user"` resolve to `<repo>/types/user.ts`.
    /// Pre-fix this hit the early `if !import_source.starts_with('.')` return
    /// and dropped through to the sidecar with a literal `types/user`, which
    /// then failed `fs.existsSync` and emitted "Source file not found".
    #[test]
    fn test_resolve_import_path_uses_tsconfig_baseurl_for_bare_specifier() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(
            repo.path().join("tsconfig.json"),
            r#"{ "compilerOptions": { "baseUrl": "." } }"#,
        )
        .unwrap();
        std::fs::create_dir_all(repo.path().join("types")).unwrap();
        std::fs::write(
            repo.path().join("types/user.ts"),
            "export interface User { id: number }",
        )
        .unwrap();
        let server = repo.path().join("server.ts");
        std::fs::write(&server, "// stub").unwrap();

        let resolved =
            FileOrchestrator::resolve_import_path(server.to_string_lossy().as_ref(), "types/user");

        let expected = repo.path().join("types/user.ts").canonicalize().unwrap();
        assert_eq!(
            std::path::Path::new(&resolved).canonicalize().unwrap(),
            expected,
            "bare specifier should resolve via baseUrl, not fall through"
        );
    }

    /// Bare specifiers that aren't on disk (real node_modules packages like
    /// `react`) must still pass through unchanged so downstream code can
    /// distinguish package imports from missing local files.
    #[test]
    fn test_resolve_import_path_preserves_unresolvable_bare_specifier() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(
            repo.path().join("tsconfig.json"),
            r#"{ "compilerOptions": { "baseUrl": "." } }"#,
        )
        .unwrap();
        let server = repo.path().join("server.ts");
        std::fs::write(&server, "// stub").unwrap();

        let resolved =
            FileOrchestrator::resolve_import_path(server.to_string_lossy().as_ref(), "react");

        assert_eq!(resolved, "react");
    }

    /// `tsc` only enables baseUrl-based non-relative resolution when the
    /// option is explicitly set. A tsconfig without `baseUrl` must not
    /// shadow real package imports — bare specifiers should pass through.
    #[test]
    fn test_resolve_import_path_skips_baseurl_when_not_set() {
        let repo = tempfile::tempdir().unwrap();
        // tsconfig WITHOUT baseUrl
        std::fs::write(
            repo.path().join("tsconfig.json"),
            r#"{ "compilerOptions": { "strict": true } }"#,
        )
        .unwrap();
        // A file at types/user.ts that *would* resolve if we defaulted
        // baseUrl to "." — must NOT be picked up here.
        std::fs::create_dir_all(repo.path().join("types")).unwrap();
        std::fs::write(
            repo.path().join("types/user.ts"),
            "export interface User { id: number }",
        )
        .unwrap();
        let server = repo.path().join("server.ts");
        std::fs::write(&server, "// stub").unwrap();

        let resolved =
            FileOrchestrator::resolve_import_path(server.to_string_lossy().as_ref(), "types/user");

        assert_eq!(
            resolved, "types/user",
            "without explicit baseUrl, bare specifiers must pass through unchanged",
        );
    }

    /// Pre-fix, a relative import like `./foo.ts` whose target couldn't be
    /// canonicalized (broken symlink, absent file, permissions) fell through
    /// to a `.ts.ts` double-extension fallback because the wrapper helper
    /// returned `None` for already-extension paths and the outer code
    /// blindly appended `.ts`.
    #[test]
    fn test_resolve_import_path_no_double_extension_for_missing_relative() {
        let repo = tempfile::tempdir().unwrap();
        let server = repo.path().join("server.ts");
        std::fs::write(&server, "// stub").unwrap();

        let resolved = FileOrchestrator::resolve_import_path(
            server.to_string_lossy().as_ref(),
            "./missing.ts",
        );

        assert!(
            !resolved.ends_with(".ts.ts"),
            "relative import with extension must not get .ts appended on miss; got `{}`",
            resolved
        );
        assert!(
            resolved.ends_with(".ts"),
            "should still surface a single-`.ts` path; got `{}`",
            resolved
        );
    }

    /// Relative imports continue to resolve against the importing file's
    /// directory, not against tsconfig.baseUrl.
    #[test]
    fn test_resolve_import_path_relative_imports_unaffected() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("src/types")).unwrap();
        std::fs::write(
            repo.path().join("src/types/order.ts"),
            "export interface Order {}",
        )
        .unwrap();
        let server = repo.path().join("src/server.ts");
        std::fs::write(&server, "// stub").unwrap();

        let resolved = FileOrchestrator::resolve_import_path(
            server.to_string_lossy().as_ref(),
            "./types/order",
        );

        let expected = repo
            .path()
            .join("src/types/order.ts")
            .canonicalize()
            .unwrap();
        assert_eq!(
            std::path::Path::new(&resolved).canonicalize().unwrap(),
            expected,
        );
    }

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
        let agent_service = AgentService::new();
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
                    payload_expression_text: None,
                    payload_expression_line: None,
                    response_expression_text: None,
                    response_expression_line: None,
                    emission_style: None,
                    primary_type_symbol: None,
                    type_import_source: None,
                }],
                data_calls: vec![],
                graphql_operations: vec![],
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
        let agent_service = AgentService::new();
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();
        file_results.insert(
            "src/service.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![DataCallResult {
                    call_kind: None,
                    candidate_id: "span:200-260".to_string(),
                    line_number: 15,
                    target: "https://api.example.com/data".to_string(),
                    method: Some("POST".to_string()),
                    pattern_matched: "fetch(".to_string(),
                    call_expression_span_start: None,
                    call_expression_span_end: None,
                    call_expression_text: None,
                    call_expression_line: None,
                    payload_expression_text: None,
                    payload_expression_line: None,
                    primary_type_symbol: None,
                    type_import_source: None,
                }],
                graphql_operations: vec![],
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
        let agent_service = AgentService::new();
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();
        file_results.insert(
            "src/service.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![
                    DataCallResult {
                        call_kind: None,
                        candidate_id: "span:300-340".to_string(),
                        line_number: 12,
                        target: "ordersResp".to_string(),
                        method: Some("GET".to_string()),
                        pattern_matched: "resp.json()".to_string(),
                        call_expression_span_start: None,
                        call_expression_span_end: None,
                        call_expression_text: None,
                        call_expression_line: None,
                        payload_expression_text: None,
                        payload_expression_line: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    },
                    DataCallResult {
                        call_kind: None,
                        candidate_id: "span:350-400".to_string(),
                        line_number: 15,
                        target: "https://api.example.com/data".to_string(),
                        method: Some("GET".to_string()),
                        pattern_matched: "fetch(".to_string(),
                        call_expression_span_start: Some(350),
                        call_expression_span_end: Some(400),
                        call_expression_text: None,
                        call_expression_line: None,
                        payload_expression_text: None,
                        payload_expression_line: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    },
                ],
                graphql_operations: vec![],
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
        let agent_service = AgentService::new();
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();
        file_results.insert(
            "src/service.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![DataCallResult {
                    call_kind: None,
                    candidate_id: "span:410-460".to_string(),
                    line_number: 12,
                    target: "https://api.example.com/data".to_string(),
                    method: Some(".json()".to_string()),
                    pattern_matched: "resp.json()".to_string(),
                    call_expression_span_start: None,
                    call_expression_span_end: None,
                    call_expression_text: None,
                    call_expression_line: None,
                    payload_expression_text: None,
                    payload_expression_line: None,
                    primary_type_symbol: None,
                    type_import_source: None,
                }],
                graphql_operations: vec![],
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
        let agent_service = AgentService::new();
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();
        file_results.insert(
            "src/service.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![],
                data_calls: vec![
                    DataCallResult {
                        call_kind: None,
                        candidate_id: "span:470-520".to_string(),
                        line_number: 10,
                        target: "https://api.example.com/orders".to_string(),
                        method: Some("GET".to_string()),
                        pattern_matched: "fetch(".to_string(),
                        call_expression_span_start: Some(470),
                        call_expression_span_end: Some(520),
                        call_expression_text: None,
                        call_expression_line: None,
                        payload_expression_text: None,
                        payload_expression_line: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    },
                    DataCallResult {
                        call_kind: None,
                        candidate_id: "span:530-580".to_string(),
                        line_number: 20,
                        target: "https://api.example.com/orders".to_string(),
                        method: Some("GET".to_string()),
                        pattern_matched: "fetch(".to_string(),
                        call_expression_span_start: Some(530),
                        call_expression_span_end: Some(580),
                        call_expression_text: None,
                        call_expression_line: None,
                        payload_expression_text: None,
                        payload_expression_line: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    },
                ],
                graphql_operations: vec![],
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
    fn test_collect_type_requests_file_based_route_uses_function_return() {
        // A file-based route endpoint (sentinel owner) carries a handler span but
        // no call-site payload expression. Its response type must be requested as
        // a line-anchored FunctionReturn (the handler's return type), NOT a
        // span/text ResponseBody — which would misread the function declaration.
        let agent_service = AgentService::new();
        let orchestrator = FileOrchestrator::new(agent_service);

        let mut file_results = HashMap::new();
        file_results.insert(
            "app/users/route.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![EndpointResult {
                    candidate_id: "file-route:GET:42".to_string(),
                    line_number: 7,
                    owner_node: FILE_BASED_ROUTE_OWNER.to_string(),
                    method: "GET".to_string(),
                    path: "/users".to_string(),
                    handler_name: "GET".to_string(),
                    pattern_matched: "nextjs-app".to_string(),
                    // Span points at the whole handler declaration — the landmine
                    // the old code would have fed to the response-body locator.
                    call_expression_span_start: Some(42),
                    call_expression_span_end: Some(300),
                    payload_expression_text: None,
                    payload_expression_line: None,
                    response_expression_text: None,
                    response_expression_line: None,
                    emission_style: None,
                    primary_type_symbol: None,
                    type_import_source: None,
                }],
                data_calls: vec![],
                graphql_operations: vec![],
            },
        );

        let graph = orchestrator.build_mount_graph(&file_results);
        let config = Config::default();
        let (_explicit, infer, _inline) =
            orchestrator.collect_type_requests(&file_results, ".", &graph, &config);

        // Exactly one inference: the response. No request-body inference for a
        // file-based GET (and none even for POST — not recoverable from the sig).
        assert_eq!(infer.len(), 1);
        let item = &infer[0];
        assert_eq!(item.infer_kind, InferKind::FunctionReturn);
        assert_eq!(item.line_number, 7);
        // Line-only locator: no span, no text — so the sidecar uses findFunctionByLine
        // and can't misresolve the declaration span as a payload.
        assert!(item.span_start.is_none());
        assert!(item.span_end.is_none());
        assert!(item.expression_text.is_none());
        let alias = item.alias.as_deref().unwrap_or_default();
        assert!(alias.contains("Response"), "alias was {alias}");
    }

    /// Build a call-site endpoint with the given emission style. Carries both
    /// a response expression and SWC spans so the test proves the routing
    /// decision comes from `emission_style`, not from locator availability.
    fn endpoint_with_emission_style(
        method: &str,
        path: &str,
        emission_style: Option<EmissionStyle>,
    ) -> EndpointResult {
        EndpointResult {
            candidate_id: "span:100-200".to_string(),
            line_number: 12,
            owner_node: "app".to_string(),
            method: method.to_string(),
            path: path.to_string(),
            handler_name: "anonymous".to_string(),
            pattern_matched: ".get(".to_string(),
            call_expression_span_start: Some(100),
            call_expression_span_end: Some(200),
            payload_expression_text: None,
            payload_expression_line: None,
            response_expression_text: Some("users".to_string()),
            response_expression_line: Some(13),
            emission_style,
            primary_type_symbol: None,
            type_import_source: None,
        }
    }

    type CollectedRequests = (
        Vec<SymbolRequest>,
        Vec<InferRequestItem>,
        Vec<(String, String)>,
    );

    fn collect_for_endpoint(endpoint: EndpointResult) -> CollectedRequests {
        let orchestrator = FileOrchestrator::new(AgentService::new());
        let mut file_results = HashMap::new();
        file_results.insert(
            "src/server.ts".to_string(),
            FileAnalysisResult {
                mounts: vec![],
                endpoints: vec![endpoint],
                data_calls: vec![],
                graphql_operations: vec![],
            },
        );
        let graph = orchestrator.build_mount_graph(&file_results);
        let config = Config::default();
        orchestrator.collect_type_requests(&file_results, ".", &graph, &config)
    }

    fn infer_for_endpoint(endpoint: EndpointResult) -> Vec<InferRequestItem> {
        collect_for_endpoint(endpoint).1
    }

    #[test]
    fn test_collect_type_requests_return_value_uses_function_return_via_text() {
        // Fastify return-style: the handler's return value IS the payload.
        // With a response expression available, the request must be a
        // text-located FunctionReturn — the sidecar resolves the expression's
        // containing function, which finds the exact handler even when it's a
        // named reference declared far from the registration line.
        let infer = infer_for_endpoint(endpoint_with_emission_style(
            "GET",
            "/users",
            Some(EmissionStyle::ReturnValue),
        ));

        assert_eq!(infer.len(), 1);
        let item = &infer[0];
        assert_eq!(item.infer_kind, InferKind::FunctionReturn);
        assert_eq!(item.line_number, 12);
        assert_eq!(item.expression_text.as_deref(), Some("users"));
        assert_eq!(item.expression_line, Some(13));
        assert!(
            item.span_start.is_none() && item.span_end.is_none(),
            "the call-expression span must not be sent — the registration call \
             contains the handler, so span resolution would bind the wrong function: {:?}",
            item
        );
    }

    #[test]
    fn test_collect_type_requests_return_value_falls_back_to_line_anchor() {
        // Pairing-invariant violation (return-value but no expression): fall
        // back to anchoring on the registration line, which is correct for
        // inline handlers (their function starts on that line).
        let mut endpoint =
            endpoint_with_emission_style("GET", "/users", Some(EmissionStyle::ReturnValue));
        endpoint.response_expression_text = None;
        endpoint.response_expression_line = None;

        let infer = infer_for_endpoint(endpoint);
        assert_eq!(infer.len(), 1);
        let item = &infer[0];
        assert_eq!(item.infer_kind, InferKind::FunctionReturn);
        assert_eq!(item.line_number, 12);
        assert!(
            item.span_start.is_none() && item.span_end.is_none() && item.expression_text.is_none(),
            "fallback must be line-anchored only: {:?}",
            item
        );
    }

    #[test]
    fn test_collect_type_requests_no_payload_skips_response_inference() {
        // Zero-arg sends / helper-written payloads: no recoverable payload
        // expression exists, so no inference is requested at all — the
        // manifest entry stays honestly `unknown` with its evidence. The
        // spans on the endpoint are the landmine: without the emission_style
        // gate the span fallback would infer from the whole `app.get(...)`
        // call expression.
        let mut endpoint =
            endpoint_with_emission_style("GET", "/export", Some(EmissionStyle::NoPayload));
        // Pairing invariant: no-payload ⇒ response expression is null.
        endpoint.response_expression_text = None;
        endpoint.response_expression_line = None;

        let infer = infer_for_endpoint(endpoint);
        assert!(
            infer.is_empty(),
            "no-payload endpoints must not request response inference: {:?}",
            infer
        );
    }

    #[test]
    fn test_collect_type_requests_no_payload_skips_explicit_symbol_too() {
        // A no-payload endpoint may still carry a (validated) type hint the
        // model picked up from imports — but the handler never sends it, so
        // bundling it would publish a phantom response contract. Both the
        // explicit-symbol path and the inline-alias fallback must be gated,
        // not just inference.
        let mut endpoint =
            endpoint_with_emission_style("GET", "/export", Some(EmissionStyle::NoPayload));
        endpoint.response_expression_text = None;
        endpoint.response_expression_line = None;
        endpoint.primary_type_symbol = Some("User".to_string());
        endpoint.type_import_source = Some("./types".to_string());

        let (explicit, infer, inline) = collect_for_endpoint(endpoint);
        assert!(
            explicit.is_empty(),
            "no-payload endpoints must not bundle explicit response symbols: {:?}",
            explicit
        );
        assert!(infer.is_empty(), "got: {:?}", infer);
        assert!(inline.is_empty(), "got: {:?}", inline);
    }

    #[test]
    fn test_collect_type_requests_no_payload_keeps_request_body_inference() {
        // The classification is about the RESPONSE payload; request-body
        // inference for mutating methods is unaffected.
        let mut endpoint =
            endpoint_with_emission_style("POST", "/orders", Some(EmissionStyle::NoPayload));
        endpoint.response_expression_text = None;
        endpoint.response_expression_line = None;
        endpoint.payload_expression_text = Some("req.body".to_string());
        endpoint.payload_expression_line = Some(13);

        let infer = infer_for_endpoint(endpoint);
        assert_eq!(infer.len(), 1, "got: {:?}", infer);
        assert_eq!(infer[0].infer_kind, InferKind::RequestBody);
        assert_eq!(infer[0].expression_text.as_deref(), Some("req.body"));
    }

    #[test]
    fn test_collect_type_requests_imperative_send_matches_legacy_default() {
        // Explicit imperative-send and an absent emission_style (cached
        // pre-emission-style analysis) must produce the identical request:
        // text-located ResponseBody.
        for style in [Some(EmissionStyle::ImperativeSend), None] {
            let infer = infer_for_endpoint(endpoint_with_emission_style("GET", "/users", style));
            assert_eq!(infer.len(), 1, "style {:?} got: {:?}", style, infer);
            let item = &infer[0];
            assert_eq!(item.infer_kind, InferKind::ResponseBody);
            assert_eq!(item.expression_text.as_deref(), Some("users"));
            assert_eq!(item.expression_line, Some(13));
        }
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
                    payload_expression_text: None,
                    payload_expression_line: None,
                    response_expression_text: None,
                    response_expression_line: None,
                    emission_style: None,
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
                    payload_expression_text: None,
                    payload_expression_line: None,
                    response_expression_text: None,
                    response_expression_line: None,
                    emission_style: None,
                    primary_type_symbol: Some("Models.User".to_string()),
                    type_import_source: Some("./models".to_string()),
                },
            ],
            data_calls: vec![DataCallResult {
                call_kind: None,
                candidate_id: "span:660-700".to_string(),
                line_number: 12,
                target: "/users".to_string(),
                method: Some("GET".to_string()),
                pattern_matched: "fetch(".to_string(),
                call_expression_span_start: None,
                call_expression_span_end: None,
                call_expression_text: None,
                call_expression_line: None,
                payload_expression_text: None,
                payload_expression_line: None,
                primary_type_symbol: Some("LocalType".to_string()),
                type_import_source: None,
            }],
            graphql_operations: vec![],
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
        let agent_service = AgentService::new();
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
                graphql_operations: vec![],
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
                        payload_expression_text: None,
                        payload_expression_line: None,
                        response_expression_text: None,
                        response_expression_line: None,
                        emission_style: None,
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
                        payload_expression_text: None,
                        payload_expression_line: None,
                        response_expression_text: None,
                        response_expression_line: None,
                        emission_style: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    },
                ],
                data_calls: vec![],
                graphql_operations: vec![],
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
        let agent_service = AgentService::new();
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
        assert_eq!(stats.file_based_endpoints, 0);
        assert!(stats.errors.is_empty());
    }

    fn next_conventions() -> Vec<RoutingConvention> {
        builtin_conventions(&["Next.js".to_string()])
    }

    #[test]
    fn test_file_based_endpoints_app_router_method_per_export() {
        let scanner = SwcScanner::new();
        let content = r#"
export async function GET() { return Response.json([]); }
export async function POST(req: Request) { return Response.json({}); }
export const runtime = "edge";
"#;
        let mut endpoints = FileOrchestrator::file_based_endpoints(
            &scanner,
            Path::new("app/users/route.ts"),
            Path::new("app/users/route.ts"),
            content,
            &next_conventions(),
        );
        endpoints.sort_by(|a, b| a.method.cmp(&b.method));

        // GET + POST become endpoints; `runtime` is not an HTTP method.
        assert_eq!(endpoints.len(), 2, "expected GET and POST only");
        assert_eq!(endpoints[0].method, "GET");
        assert_eq!(endpoints[1].method, "POST");
        for ep in &endpoints {
            assert_eq!(ep.path, "/users");
            assert_eq!(ep.owner_node, FILE_BASED_ROUTE_OWNER);
            assert_eq!(ep.pattern_matched, "nextjs-app");
            assert!(ep.call_expression_span_start.is_some());
            assert!(ep.call_expression_span_end.is_some());
            // Type enrichment is deferred to the LLM/sidecar pass.
            assert!(ep.response_expression_text.is_none());
        }
    }

    #[test]
    fn test_file_based_endpoints_dynamic_segment() {
        let scanner = SwcScanner::new();
        let content = "export async function GET() {}\n";
        let endpoints = FileOrchestrator::file_based_endpoints(
            &scanner,
            Path::new("app/users/[id]/route.ts"),
            Path::new("app/users/[id]/route.ts"),
            content,
            &next_conventions(),
        );
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].method, "GET");
        assert_eq!(endpoints[0].path, "/users/:id");
    }

    #[test]
    fn test_file_based_endpoints_astro_filename_with_export_methods() {
        // Astro is the FileName + ExportName combination: the path comes from
        // the filename (like pages-router) but methods come from named exports
        // (like app-router). Both `export function` and `export const` forms
        // must be recognized.
        let scanner = SwcScanner::new();
        let content = r#"
export async function GET() { return new Response("[]"); }
export const POST = async (ctx) => new Response("{}");
export const prerender = false;
"#;
        let mut endpoints = FileOrchestrator::file_based_endpoints(
            &scanner,
            Path::new("src/pages/api/users.ts"),
            Path::new("src/pages/api/users.ts"),
            content,
            &builtin_conventions(&["Astro".to_string()]),
        );
        endpoints.sort_by(|a, b| a.method.cmp(&b.method));

        // GET + POST become endpoints; `prerender` is not an HTTP method.
        assert_eq!(endpoints.len(), 2, "expected GET and POST only");
        assert_eq!(endpoints[0].method, "GET");
        assert_eq!(endpoints[1].method, "POST");
        for ep in &endpoints {
            assert_eq!(ep.path, "/api/users");
            assert_eq!(ep.owner_node, FILE_BASED_ROUTE_OWNER);
            assert_eq!(ep.pattern_matched, "astro");
        }
    }

    #[test]
    fn test_file_based_endpoints_astro_dynamic_segment() {
        let scanner = SwcScanner::new();
        let content = "export function GET() {}\n";
        let endpoints = FileOrchestrator::file_based_endpoints(
            &scanner,
            Path::new("src/pages/posts/[id].ts"),
            Path::new("src/pages/posts/[id].ts"),
            content,
            &builtin_conventions(&["Astro".to_string()]),
        );
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].method, "GET");
        assert_eq!(endpoints[0].path, "/posts/:id");
    }

    #[test]
    fn test_file_based_endpoints_pages_router_default_export_deferred() {
        // Pages-router default export serves every method; the method set isn't
        // recoverable from the layout, so no endpoint is synthesized (yet).
        let scanner = SwcScanner::new();
        let content = "export default function handler(req, res) {}\n";
        let endpoints = FileOrchestrator::file_based_endpoints(
            &scanner,
            Path::new("pages/api/users.ts"),
            Path::new("pages/api/users.ts"),
            content,
            &next_conventions(),
        );
        assert!(endpoints.is_empty());
    }

    #[test]
    fn test_file_based_endpoints_non_route_file() {
        let scanner = SwcScanner::new();
        let content = "export async function GET() {}\n";
        let endpoints = FileOrchestrator::file_based_endpoints(
            &scanner,
            Path::new("src/services/users.ts"),
            Path::new("src/services/users.ts"),
            content,
            &next_conventions(),
        );
        assert!(
            endpoints.is_empty(),
            "non-route files should yield no file-based endpoints"
        );
    }

    #[test]
    fn test_file_based_endpoints_no_conventions_is_noop() {
        let scanner = SwcScanner::new();
        let content = "export async function GET() {}\n";
        // No convention-bearing framework detected → empty conventions.
        let endpoints = FileOrchestrator::file_based_endpoints(
            &scanner,
            Path::new("app/users/route.ts"),
            Path::new("app/users/route.ts"),
            content,
            &builtin_conventions(&["express".to_string()]),
        );
        assert!(endpoints.is_empty());
    }

    #[test]
    fn route_descriptor_endpoint_owner_is_handler_not_method() {
        // #234: a route declared as data is emitted deterministically with the
        // handler identifier as owner — never the HTTP-method literal "GET"
        // (the owner-fabrication trap, #227). Drives the real fixture.
        let scanner = SwcScanner::new();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "tests/fixtures/xrepo-corpus-1/orders-monorepo/packages/gateway/src/health.handler.ts",
        );
        let content = std::fs::read_to_string(&fixture).expect("fixture must exist");

        let endpoints = FileOrchestrator::route_descriptor_endpoints(&scanner, &fixture, &content);

        assert_eq!(
            endpoints.len(),
            1,
            "expected exactly one route-descriptor endpoint, got {endpoints:?}"
        );
        let ep = &endpoints[0];
        assert_eq!(ep.method, "GET");
        assert_eq!(ep.path, "/gateway/health");
        assert_eq!(
            ep.owner_node, "healthCheckHandler",
            "owner must be the handler ident, not the method literal"
        );
        assert_ne!(ep.owner_node, "GET");
        assert_eq!(ep.handler_name, "healthCheckHandler");
        assert_eq!(ep.pattern_matched, ROUTE_DESCRIPTOR_PATTERN);
        assert!(ep.call_expression_span_start.is_some());
        assert!(ep.call_expression_span_end.is_some());
    }

    #[test]
    fn route_descriptor_endpoint_missing_handler_uses_sentinel_owner() {
        let scanner = SwcScanner::new();
        let content = r#"
const routes = [
  { method: 'POST', path: '/widgets' },
];
export { routes };
"#;
        let endpoints =
            FileOrchestrator::route_descriptor_endpoints(&scanner, Path::new("routes.ts"), content);
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].method, "POST");
        assert_eq!(endpoints[0].path, "/widgets");
        assert_eq!(endpoints[0].owner_node, ROUTE_DESCRIPTOR_OWNER);
    }

    fn synthetic_endpoint(method: &str, path: &str) -> EndpointResult {
        EndpointResult {
            candidate_id: format!("file-route:{}:0", method),
            line_number: 1,
            owner_node: FILE_BASED_ROUTE_OWNER.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            handler_name: method.to_string(),
            pattern_matched: "nextjs-app".to_string(),
            call_expression_span_start: Some(0),
            call_expression_span_end: Some(1),
            payload_expression_text: None,
            payload_expression_line: None,
            response_expression_text: None,
            response_expression_line: None,
            emission_style: None,
            primary_type_symbol: None,
            type_import_source: None,
        }
    }

    #[test]
    fn test_merge_file_based_endpoints_dedups_by_method_and_path() {
        let mut result = FileAnalysisResult {
            // The LLM pass already produced GET /users (e.g. via a Response.json
            // candidate). The structural entry for it must not be duplicated.
            endpoints: vec![synthetic_endpoint("GET", "/users")],
            ..Default::default()
        };

        let added = FileOrchestrator::merge_file_based_endpoints(
            &mut result,
            vec![
                synthetic_endpoint("get", "/users"), // duplicate (case-insensitive method)
                synthetic_endpoint("POST", "/users"), // new method, same path
            ],
        );

        assert_eq!(added, 1);
        assert_eq!(result.endpoints.len(), 2);
        assert!(
            result
                .endpoints
                .iter()
                .any(|e| e.method == "POST" && e.path == "/users")
        );
    }

    #[test]
    fn test_canonicalize_route_path_brackets_to_colons() {
        // Astro/Next bracket params -> colon form; catch-alls -> **; colon and
        // literal segments are untouched (idempotent).
        assert_eq!(
            FileOrchestrator::canonicalize_route_path("/w/[slug]/projects/new"),
            "/w/:slug/projects/new"
        );
        assert_eq!(
            FileOrchestrator::canonicalize_route_path("/w/[slug]/p/[projSlug]/keys/new"),
            "/w/:slug/p/:projSlug/keys/new"
        );
        assert_eq!(
            FileOrchestrator::canonicalize_route_path("/files/[...path]"),
            "/files/**"
        );
        // Next.js optional catch-all must also reach `**`, matching the router,
        // not a malformed `:[slug]`.
        assert_eq!(
            FileOrchestrator::canonicalize_route_path("/blog/[[...slug]]"),
            "/blog/**"
        );
        // Whitespace-jittered brackets must still dedupe against the router's
        // trimmed colon form (Copilot review).
        assert_eq!(
            FileOrchestrator::canonicalize_route_path("/w/[ slug ]/x"),
            "/w/:slug/x"
        );
        assert_eq!(
            FileOrchestrator::canonicalize_route_path("/w/[[ id ]]/x"),
            "/w/:id/x"
        );
        assert_eq!(
            FileOrchestrator::canonicalize_route_path("/w/:slug/invite"),
            "/w/:slug/invite"
        );
    }

    #[test]
    fn test_canonicalize_collapses_bracket_and_colon_duplicate() {
        // The LLM emitted the bracket form; the file-based router emits the colon
        // form. After canonicalization they are the same path, so the structural
        // entry dedupes instead of producing a second, form-flipped endpoint.
        let mut result = FileAnalysisResult {
            endpoints: vec![synthetic_endpoint("POST", "/w/[slug]/projects/new")],
            ..Default::default()
        };
        FileOrchestrator::canonicalize_endpoint_paths(&mut result);
        assert_eq!(result.endpoints[0].path, "/w/:slug/projects/new");

        let added = FileOrchestrator::merge_file_based_endpoints(
            &mut result,
            vec![synthetic_endpoint("POST", "/w/:slug/projects/new")],
        );
        assert_eq!(
            added, 0,
            "colon-form route should dedupe against the canonicalized LLM path"
        );
        assert_eq!(result.endpoints.len(), 1);
    }
}
