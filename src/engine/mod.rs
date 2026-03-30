use crate::agent_service::AgentService;
use crate::agents::file_orchestrator::FileOrchestrator;
use crate::agents::framework_guidance_agent::{FrameworkGuidance, FrameworkGuidanceAgent};
use crate::analyzer::{Analyzer, ApiEndpointDetails, builder::AnalyzerBuilder};
use crate::cloud_storage::{
    CloudRepoData, CloudStorage, ManifestRole, ManifestTypeKind, ManifestTypeState,
    TypeManifestEntry, get_current_commit_hash,
};
use crate::config::{Config, create_dynamic_tsconfig};
use crate::file_finder::find_files;
use crate::framework_detector::{DetectionResult, FrameworkDetector};
use crate::mount_graph::MountGraph;
use crate::multi_agent_orchestrator::MultiAgentOrchestrator;
use crate::packages::Packages;
use crate::parser::parse_file;
use crate::services::{
    TypeSidecar,
    type_sidecar::{InferKind, TypeResolutionResult},
};
use crate::type_manifest::{
    build_call_site_id, build_manifest_type_alias_with_call_id, is_http_method,
    normalize_manifest_method, parse_file_location,
};
use crate::url_normalizer::UrlNormalizer;
use crate::utils::get_repository_name;
use crate::visitor::{FunctionDefinition, FunctionDefinitionExtractor, ImportSymbolExtractor};
use crate::workspace::detect_workspace;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;

/// Current cache format version. Increment when FileAnalysisResult schema changes.
const CACHE_VERSION: u32 = 1;

// Type aliases to reduce complexity
type FileDiscoveryResult = Result<
    (
        Vec<PathBuf>,
        HashMap<String, crate::visitor::ImportedSymbol>,
        HashMap<String, FunctionDefinition>,
        String,
        Option<PathBuf>, // config file path
        Option<PathBuf>, // package.json path
    ),
    Box<dyn std::error::Error>,
>;

/// Determine if we should upload data based on GitHub context
/// Only upload on main/master branch, not on PRs
fn should_upload_data() -> bool {
    // Check if we're in a pull request
    if let Ok(event_name) = env::var("GITHUB_EVENT_NAME") {
        if event_name == "pull_request" {
            return false;
        }
    }

    // Check if we're on a feature branch (not main/master)
    if let Ok(ref_name) = env::var("GITHUB_REF") {
        // GITHUB_REF format: refs/heads/branch-name or refs/pull/123/merge
        if ref_name.starts_with("refs/pull/") {
            return false;
        }

        if let Some(branch) = ref_name.strip_prefix("refs/heads/") {
            // Only upload for main/master branches
            return branch == "main" || branch == "master";
        }
    }

    // If we can't determine the context, default to upload (for local testing)
    // You might want to change this to false for stricter behavior
    true
}

#[allow(dead_code)]
pub async fn run_analysis_engine<T: CloudStorage>(
    storage: T,
    repo_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    run_analysis_engine_with_sidecar(storage, repo_path, None).await
}

/// Run analysis engine with optional sidecar for type extraction
pub async fn run_analysis_engine_with_sidecar<T: CloudStorage>(
    storage: T,
    repo_path: &str,
    sidecar: Option<&TypeSidecar>,
) -> Result<(), Box<dyn std::error::Error>> {
    let carrick_org = env::var("CARRICK_ORG").map_err(|_| "CARRICK_ORG must be set in CI mode")?;

    let should_upload = should_upload_data();
    println!(
        "Running Carrick in CI mode with org: {} (upload: {})",
        &carrick_org, should_upload
    );

    // 1. Health check
    storage
        .health_check()
        .await
        .map_err(|e| format!("Failed to connect to AWS services: {}", e))?;
    println!("AWS connectivity verified");

    // 2. Download all repos (moved earlier for incremental cache lookup)
    let (mut all_repo_data, _repo_s3_urls) = storage
        .download_all_repo_data(&carrick_org)
        .await
        .map_err(|e| format!("Failed to download cross-repo data: {}", e))?;

    let repo_name = get_repository_name(repo_path);
    let workspace = detect_workspace(repo_path);

    if workspace.is_monorepo {
        println!(
            "[workspace] Detected monorepo with {} packages:",
            workspace.packages.len()
        );
        for pkg in &workspace.packages {
            println!("  - {} ({})", pkg.name, pkg.path.display());
        }

        let mut all_package_data: Vec<CloudRepoData> = Vec::new();

        for package in &workspace.packages {
            let composite_name = format!("{}::{}", repo_name, package.name);
            println!(
                "\n=== Analyzing package: {} ({}) ===",
                package.name,
                package.path.display()
            );

            let previous_data = all_repo_data
                .iter()
                .find(|r| r.repo_name == composite_name)
                .cloned();

            if previous_data.is_some() {
                println!(
                    "[incremental] Found previous analysis data for {}",
                    composite_name
                );
            }

            let package_path = package.path.to_string_lossy().to_string();
            let mut package_data = analyze_current_repo_incremental(
                &package_path,
                sidecar,
                previous_data.as_ref(),
                Some(repo_path),
            )
            .await?;

            package_data.repo_name = composite_name;
            package_data.package_name = Some(package.name.clone());

            println!(
                "Analyzed package {}: {} endpoints, {} calls",
                package.name,
                package_data.endpoints.len(),
                package_data.calls.len()
            );

            if should_upload {
                let cloud_data_serialized = strip_ast_nodes(package_data.clone());
                storage
                    .upload_repo_data(&carrick_org, &cloud_data_serialized)
                    .await
                    .map_err(|e| {
                        format!("Failed to upload data for package {}: {}", package.name, e)
                    })?;
                println!("Uploaded data for package {}", package.name);
            }

            all_package_data.push(package_data);
        }

        if !should_upload {
            println!("Skipping upload (PR/branch mode - analyzing only)");
        }

        let composite_prefix = format!("{}::", repo_name);
        all_repo_data.retain(|repo| {
            !repo.repo_name.starts_with(&composite_prefix) && repo.repo_name != repo_name
        });

        println!(
            "\nDownloaded data from {} external repos",
            all_repo_data.len()
        );

        if !all_package_data.is_empty() {
            let first_package = all_package_data.remove(0);
            all_repo_data.extend(all_package_data);

            let analyzer = build_cross_repo_analyzer(all_repo_data, first_package).await?;
            println!("Reconstructed analyzer with cross-repo data");

            let results = analyzer.get_results();
            print_results(results);
        } else {
            println!("No packages to analyze in monorepo");
        }
    } else {
        let previous_data = all_repo_data
            .iter()
            .find(|r| r.repo_name == repo_name)
            .cloned();

        if previous_data.is_some() {
            println!(
                "[incremental] Found previous analysis data for {}",
                repo_name
            );
        }

        let current_repo_data =
            analyze_current_repo_incremental(repo_path, sidecar, previous_data.as_ref(), None)
                .await?;
        println!("Analyzed current repo: {}", current_repo_data.repo_name);

        if current_repo_data.bundled_types.is_some() {
            println!(
                "Type resolution: {} bundled types, {} manifest entries",
                current_repo_data
                    .bundled_types
                    .as_ref()
                    .map(|s| s.lines().count())
                    .unwrap_or(0),
                current_repo_data
                    .type_manifest
                    .as_ref()
                    .map(|v| v.len())
                    .unwrap_or(0)
            );
        }

        if should_upload {
            let cloud_data_serialized = strip_ast_nodes(current_repo_data.clone());
            storage
                .upload_repo_data(&carrick_org, &cloud_data_serialized)
                .await
                .map_err(|e| format!("Failed to upload repo data: {}", e))?;
            println!("Uploaded current repo data to cloud storage");
        } else {
            println!("Skipping upload (PR/branch mode - analyzing only)");
        }

        let current_repo_name = &current_repo_data.repo_name;
        all_repo_data.retain(|repo| &repo.repo_name != current_repo_name);

        println!(
            "Downloaded data from {} repos (excluding current repo: {})",
            all_repo_data.len(),
            current_repo_name
        );

        let analyzer = build_cross_repo_analyzer(all_repo_data, current_repo_data).await?;
        println!("Reconstructed analyzer with cross-repo data");

        let results = analyzer.get_results();
        print_results(results);
    }

    Ok(())
}

/// Serialize CloudRepoData without AST nodes in ApiEndpointDetails
/// Generic function to merge serialized data from repo configs
fn merge_serialized_data<T>(
    all_repo_data: &[CloudRepoData],
    extractor: fn(&CloudRepoData) -> Option<&String>,
) -> Result<T, Box<dyn std::error::Error>>
where
    T: Default + serde::de::DeserializeOwned,
{
    // Special handling for Config to properly merge all configs
    if std::any::type_name::<T>() == std::any::type_name::<crate::config::Config>() {
        let mut temp_files = Vec::new();

        // Write each config to a temporary file
        for (i, repo_data) in all_repo_data.iter().enumerate() {
            if let Some(json_str) = extractor(repo_data) {
                let temp_path = std::env::temp_dir().join(format!("carrick_config_{}.json", i));
                if std::fs::write(&temp_path, json_str).is_err() {
                    continue;
                }
                temp_files.push(temp_path);
            }
        }

        // Use Config::new to properly merge all configs
        if !temp_files.is_empty() {
            let merged_config = crate::config::Config::new(temp_files.clone()).unwrap_or_default();

            // Clean up temp files
            for temp_file in temp_files {
                let _ = std::fs::remove_file(temp_file);
            }

            // This is a bit of a hack to return the merged config as T
            // Since we know T is Config when we get here
            let config_any = Box::new(merged_config) as Box<dyn std::any::Any>;
            if let Ok(config) = config_any.downcast::<crate::config::Config>() {
                let config_json = serde_json::to_string(&*config)?;
                return Ok(serde_json::from_str(&config_json)?);
            }
        }

        return Ok(T::default());
    }

    // For non-Config types, use the first found (original behavior)
    for repo_data in all_repo_data {
        if let Some(json_str) = extractor(repo_data) {
            if let Ok(data) = serde_json::from_str::<T>(json_str) {
                return Ok(data);
            }
        }
    }
    Ok(T::default())
}

/// Remove AST nodes from CloudRepoData for serialization.
/// Also enforces payload size limit for Lambda (6MB) — drops file_results if too large.
fn strip_ast_nodes(mut data: CloudRepoData) -> CloudRepoData {
    fn strip_endpoint_ast(endpoint: &mut ApiEndpointDetails) {
        endpoint.request_type = None;
        endpoint.response_type = None;
    }

    data.endpoints.iter_mut().for_each(strip_endpoint_ast);
    data.calls.iter_mut().for_each(strip_endpoint_ast);

    // Payload size guard: Lambda function URLs have a 6MB request payload limit.
    // If serialized data exceeds ~5MB, drop file_results to stay under the limit.
    const MAX_PAYLOAD_BYTES: usize = 5 * 1024 * 1024; // 5MB safety margin
    if let Ok(serialized) = serde_json::to_string(&data) {
        if serialized.len() > MAX_PAYLOAD_BYTES {
            println!(
                "[incremental] WARNING: Payload size {}KB exceeds {}KB limit, dropping file_results cache for this upload",
                serialized.len() / 1024,
                MAX_PAYLOAD_BYTES / 1024
            );
            data.file_results = None;
            data.cached_detection = None;
            data.cached_guidance = None;
        }
    }

    data
}

/// Get files changed between a base commit and HEAD.
/// Returns relative paths matching the file discovery format.
fn get_changed_files(repo_path: &str, base_commit: &str) -> Option<Vec<String>> {
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only", base_commit, "HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!(
            "[incremental] git diff failed (shallow clone?): {}",
            stderr.trim()
        );
        println!("[incremental] Set fetch-depth: 0 in your workflow for incremental mode.");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let changed: Vec<String> = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let ext = Path::new(line)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            matches!(ext, "ts" | "tsx" | "js" | "jsx")
        })
        .map(|line| line.to_string())
        .collect();

    Some(changed)
}

/// Filter git diff paths for a monorepo package.
/// Git diff returns paths relative to the repo root (e.g., "apps/event-api/src/app.ts"),
/// but file discovery normalizes to package-relative paths (e.g., "src/app.ts").
/// This function strips the package prefix so the paths match.
fn filter_changed_files_for_package(
    changed_files: Vec<String>,
    repo_path: &str,
    repo_root: Option<&str>,
) -> HashSet<String> {
    let Some(root) = repo_root else {
        return changed_files.into_iter().collect();
    };

    let canon_root = std::fs::canonicalize(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| root.to_string());

    let package_prefix = repo_path
        .strip_prefix(&format!("{}/", canon_root))
        .or_else(|| repo_path.strip_prefix(&canon_root))
        .unwrap_or("");

    if package_prefix.is_empty() {
        eprintln!(
            "[incremental] WARNING: Could not compute package prefix (repo_path={}, root={}), \
             falling back to full diff",
            repo_path, canon_root
        );
        return changed_files.into_iter().collect();
    }

    let prefix_with_slash = format!("{}/", package_prefix);

    changed_files
        .into_iter()
        .filter_map(|f| f.strip_prefix(&prefix_with_slash).map(|s| s.to_string()))
        .collect()
}

/// Hash file content for cache invalidation (package.json).
fn hash_file_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Normalize file_results keys to be relative to repo root.
/// This ensures cache key consistency between runs.
fn normalize_file_results_keys(
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
    repo_path: &str,
) -> HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult> {
    let repo_prefix = if repo_path.ends_with('/') {
        repo_path.to_string()
    } else {
        format!("{}/", repo_path)
    };

    file_results
        .iter()
        .map(|(key, value)| {
            let normalized_key = key
                .strip_prefix(&repo_prefix)
                .or_else(|| key.strip_prefix("./"))
                .unwrap_or(key)
                .to_string();
            (normalized_key, value.clone())
        })
        .collect()
}

/// Strip diagnostic-only fields from file_results before caching.
/// These fields are not needed by build_mount_graph() or collect_type_requests().
fn strip_diagnostic_fields(
    file_results: &mut HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
) {
    for result in file_results.values_mut() {
        for endpoint in &mut result.endpoints {
            endpoint.candidate_id = String::new();
            endpoint.pattern_matched = String::new();
            endpoint.payload_expression_text = None;
            endpoint.response_expression_text = None;
        }
        for data_call in &mut result.data_calls {
            data_call.candidate_id = String::new();
            data_call.pattern_matched = String::new();
            data_call.call_expression_text = None;
            data_call.payload_expression_text = None;
        }
        for mount in &mut result.mounts {
            mount.pattern_matched = String::new();
        }
    }
}

/// Incremental analysis: reuse cached per-file LLM results for unchanged files.
/// `repo_root` is the git root directory (used for monorepo packages where repo_path
/// is a subdirectory). When None, repo_path is assumed to be the git root.
async fn analyze_current_repo_incremental(
    repo_path: &str,
    sidecar: Option<&TypeSidecar>,
    previous_data: Option<&CloudRepoData>,
    repo_root: Option<&str>,
) -> Result<CloudRepoData, Box<dyn std::error::Error>> {
    let start = Instant::now();

    // Canonicalize repo_path for consistent path normalization between runs
    let canonical = std::fs::canonicalize(repo_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| repo_path.to_string());
    let repo_path = canonical.as_str();

    let cm: Lrc<SourceMap> = Default::default();
    let (files, all_imported_symbols, function_definitions, repo_name, config_file, package_json) =
        discover_files_and_symbols(repo_path, cm.clone())?;

    let (config, packages) = load_config_and_packages(config_file, package_json)?;

    // 3. Check if we can use incremental mode
    let can_use_incremental = previous_data
        .and_then(|prev| {
            // Must have file_results and matching cache_version
            let has_cache = prev.file_results.is_some();
            let version_matches = prev.cache_version == Some(CACHE_VERSION);
            if has_cache && version_matches {
                Some(prev)
            } else {
                if !has_cache {
                    println!("[incremental] No cached file_results found, running full analysis");
                }
                if !version_matches {
                    println!(
                        "[incremental] Cache version mismatch (expected {}, got {:?}), running full analysis",
                        CACHE_VERSION,
                        prev.cache_version
                    );
                }
                None
            }
        });

    if let Some(prev) = can_use_incremental {
        let prev_commit = &prev.commit_hash;
        println!(
            "[incremental] Found previous analysis (commit {})",
            &prev_commit[..std::cmp::min(7, prev_commit.len())]
        );

        let git_dir = repo_root.unwrap_or(repo_path);
        if let Some(changed_files) = get_changed_files(git_dir, prev_commit) {
            let prev_file_results = prev.file_results.as_ref().unwrap();
            let repo_prefix = format!("{}/", repo_path);

            let normalize_path = |f: &PathBuf| -> String {
                let s = f.to_string_lossy();
                if let Some(stripped) = s.strip_prefix(&repo_prefix) {
                    stripped.to_string()
                } else if let Some(stripped) = s.strip_prefix("./") {
                    stripped.to_string()
                } else {
                    s.to_string()
                }
            };

            let current_file_set: HashSet<String> = files.iter().map(&normalize_path).collect();

            let changed_set: HashSet<String> =
                filter_changed_files_for_package(changed_files, repo_path, repo_root);

            // Partition: which files need fresh analysis?
            let files_to_analyze: Vec<PathBuf> = files
                .iter()
                .filter(|f| {
                    let relative = normalize_path(f);
                    changed_set.contains(&relative) || !prev_file_results.contains_key(&relative)
                })
                .cloned()
                .collect();

            let total_files = files.len();
            let changed_count = files_to_analyze.len();
            let reused_count = total_files - changed_count;

            println!(
                "[incremental] Detected {} changed file(s) out of {}",
                changed_count, total_files
            );
            println!(
                "[incremental] Reusing cached results for {} files",
                reused_count
            );

            // Check if package.json changed → need fresh framework detection/guidance
            // Hash raw file content (not serialized struct) for deterministic comparison
            let current_pkg_hash = std::fs::read_to_string(format!("{}/package.json", repo_path))
                .ok()
                .map(|content| hash_file_content(&content))
                .unwrap_or_default();

            let pkg_changed = prev.package_json_hash.as_deref() != Some(&current_pkg_hash);

            let api_key = env::var("CARRICK_API_KEY")
                .map_err(|_| "CARRICK_API_KEY environment variable must be set")?;

            // Get framework detection and guidance (cached or fresh)
            let (detection, guidance) = if !pkg_changed {
                if let (Some(det), Some(guid)) = (&prev.cached_detection, &prev.cached_guidance) {
                    println!("[incremental] Reusing cached framework detection and guidance");
                    (det.clone(), guid.clone())
                } else {
                    run_framework_detection_and_guidance(&api_key, &packages, &all_imported_symbols)
                        .await?
                }
            } else {
                println!("[incremental] package.json changed, re-running framework detection");
                run_framework_detection_and_guidance(&api_key, &packages, &all_imported_symbols)
                    .await?
            };

            // Run Gemini file analysis ONLY on changed files
            if changed_count > 0 {
                println!(
                    "[incremental] Running LLM analysis on {} changed file(s)...",
                    changed_count
                );
            }

            let agent_service = AgentService::new(api_key.clone());
            let file_orchestrator = FileOrchestrator::new(agent_service);

            let new_file_results = if !files_to_analyze.is_empty() {
                let result = file_orchestrator
                    .analyze_files(&files_to_analyze, &guidance, &detection)
                    .await?;
                result.file_results
            } else {
                HashMap::new()
            };

            // Merge: start with previous, remove deleted files, update changed
            let mut merged_results: HashMap<
                String,
                crate::agents::file_analyzer_agent::FileAnalysisResult,
            > = HashMap::new();

            // Copy cached results for unchanged files that still exist
            for (path, result) in prev_file_results {
                if current_file_set.contains(path) && !changed_set.contains(path) {
                    merged_results.insert(path.clone(), result.clone());
                }
            }

            // Normalize and insert new results
            let normalized_new = normalize_file_results_keys(&new_file_results, repo_path);
            for (path, result) in normalized_new {
                merged_results.insert(path, result);
            }

            // Rebuild mount graph from full merged results
            let agent_service_for_graph = AgentService::new(api_key.clone());
            let graph_orchestrator = FileOrchestrator::new(agent_service_for_graph);
            let mount_graph = graph_orchestrator.build_mount_graph(&merged_results);

            let elapsed = start.elapsed();
            println!(
                "[incremental] Analysis complete in {:.1}s",
                elapsed.as_secs_f64()
            );

            // Build CloudRepoData with merged results
            let mut cloud_data = build_cloud_data_from_mount_graph(
                &repo_name,
                repo_path,
                &mount_graph,
                &config,
                &packages,
                function_definitions,
            );

            // Populate cache fields
            let mut cached_file_results = merged_results.clone();
            strip_diagnostic_fields(&mut cached_file_results);
            cloud_data.file_results = Some(cached_file_results);
            cloud_data.cached_detection = Some(detection);
            cloud_data.cached_guidance = Some(guidance);
            cloud_data.package_json_hash = Some(current_pkg_hash);
            cloud_data.cache_version = Some(CACHE_VERSION);

            // Build type manifest
            let manifest_entries = build_type_manifest_entries(&mount_graph, &config);
            if !manifest_entries.is_empty() {
                cloud_data.type_manifest = Some(manifest_entries);
            }

            // Type resolution via sidecar
            resolve_types_if_available(
                sidecar,
                &file_orchestrator,
                &merged_results,
                repo_path,
                &packages,
                &mount_graph,
                &config,
                &mut cloud_data,
            );

            if let Some(bundled_types) = cloud_data.bundled_types.take() {
                let updated =
                    append_missing_aliases(bundled_types, cloud_data.type_manifest.as_ref());
                cloud_data.bundled_types = Some(updated);
            }

            return Ok(cloud_data);
        } else {
            println!("[incremental] git diff failed, falling back to full analysis");
        }
    }

    // Fallback: full analysis (analyze_current_repo now populates cache fields)
    println!("[incremental] Running full analysis...");
    let cloud_data = analyze_current_repo(repo_path, sidecar).await?;

    let elapsed = start.elapsed();
    println!(
        "[incremental] Full analysis complete in {:.1}s",
        elapsed.as_secs_f64()
    );

    Ok(cloud_data)
}

/// Run framework detection and guidance generation.
async fn run_framework_detection_and_guidance(
    api_key: &str,
    packages: &Packages,
    imported_symbols: &HashMap<String, crate::visitor::ImportedSymbol>,
) -> Result<(DetectionResult, FrameworkGuidance), Box<dyn std::error::Error>> {
    let agent_service = AgentService::new(api_key.to_string());
    let framework_detector = FrameworkDetector::new(agent_service.clone());
    let detection = framework_detector
        .detect_frameworks_and_libraries(packages, imported_symbols)
        .await?;

    let guidance_agent = FrameworkGuidanceAgent::new(agent_service);
    let guidance = guidance_agent.generate_guidance(&detection).await?;

    Ok((detection, guidance))
}

/// Build CloudRepoData from a mount graph (used by incremental path).
fn build_cloud_data_from_mount_graph(
    repo_name: &str,
    repo_path: &str,
    mount_graph: &MountGraph,
    config: &Config,
    packages: &Packages,
    function_definitions: HashMap<String, FunctionDefinition>,
) -> CloudRepoData {
    let config_json = serde_json::to_string(config).ok();
    let service_name = config_json.as_ref().and_then(|json| {
        serde_json::from_str::<serde_json::Value>(json)
            .ok()
            .and_then(|v| {
                v.get("serviceName")
                    .and_then(|s| s.as_str())
                    .map(String::from)
            })
    });

    let endpoints: Vec<ApiEndpointDetails> = mount_graph
        .get_resolved_endpoints()
        .iter()
        .map(|endpoint| ApiEndpointDetails {
            owner: Some(crate::visitor::OwnerType::App(endpoint.owner.clone())),
            route: endpoint.full_path.clone(),
            method: endpoint.method.clone(),
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: endpoint.handler.clone(),
            request_type: None,
            response_type: None,
            file_path: PathBuf::from(&endpoint.file_location),
        })
        .collect();

    let calls: Vec<ApiEndpointDetails> = mount_graph
        .get_data_calls()
        .iter()
        .map(|call| ApiEndpointDetails {
            owner: None,
            route: call.target_url.clone(),
            method: call.method.clone(),
            params: vec![],
            request_body: None,
            response_body: None,
            handler_name: Some(call.client.clone()),
            request_type: None,
            response_type: None,
            file_path: PathBuf::from(&call.file_location),
        })
        .collect();

    let mounts: Vec<crate::visitor::Mount> = mount_graph
        .get_mounts()
        .iter()
        .map(|mount| crate::visitor::Mount {
            parent: crate::visitor::OwnerType::App(mount.parent.clone()),
            child: crate::visitor::OwnerType::Router(mount.child.clone()),
            prefix: mount.path_prefix.clone(),
        })
        .collect();

    println!("Created CloudRepoData from incremental analysis:");
    println!("  - {} endpoints", endpoints.len());
    println!("  - {} calls", calls.len());
    println!("  - {} mounts", mounts.len());

    CloudRepoData {
        repo_name: repo_name.to_string(),
        service_name,
        package_name: None,
        endpoints,
        calls,
        mounts,
        apps: HashMap::new(),
        imported_handlers: vec![],
        function_definitions,
        config_json,
        package_json: serde_json::to_string(packages).ok(),
        packages: Some(packages.clone()),
        last_updated: chrono::Utc::now(),
        commit_hash: get_current_commit_hash(repo_path),
        mount_graph: Some(mount_graph.clone()),
        bundled_types: None,
        type_manifest: None,
        file_results: None,
        cached_detection: None,
        cached_guidance: None,
        package_json_hash: None,
        cache_version: None,
    }
}

/// Resolve types via sidecar if available (shared logic for full and incremental paths).
#[allow(clippy::too_many_arguments)]
fn resolve_types_if_available(
    sidecar: Option<&TypeSidecar>,
    file_orchestrator: &FileOrchestrator,
    file_results: &HashMap<String, crate::agents::file_analyzer_agent::FileAnalysisResult>,
    repo_path: &str,
    packages: &Packages,
    mount_graph: &MountGraph,
    config: &Config,
    cloud_data: &mut CloudRepoData,
) {
    let Some(sidecar) = sidecar else { return };

    println!("\n=== Sidecar Type Resolution ===");
    match sidecar.wait_ready(Duration::from_secs(10)) {
        Ok(()) => {
            match file_orchestrator.resolve_types_with_sidecar(
                sidecar,
                file_results,
                repo_path,
                packages,
                mount_graph,
                config,
            ) {
                Ok(type_resolution) => {
                    println!(
                        "Type resolution successful: {} explicit, {} inferred, {} failures",
                        type_resolution.explicit_manifest.len(),
                        type_resolution.inferred_types.len(),
                        type_resolution.symbol_failures.len()
                    );
                    cloud_data.bundled_types = type_resolution.dts_content.clone();
                    if let Some(ref mut manifest) = cloud_data.type_manifest {
                        enrich_manifest_with_type_resolution(
                            manifest,
                            &type_resolution,
                            type_resolution.dts_content.as_deref(),
                        );
                    }
                    for failure in &type_resolution.symbol_failures {
                        eprintln!(
                            "[Sidecar] Failed to resolve symbol '{}' from '{}': {}",
                            failure.symbol_name, failure.source_file, failure.reason
                        );
                    }
                }
                Err(e) => {
                    eprintln!("[Sidecar] Type resolution failed: {}", e);
                    eprintln!("[Sidecar] Continuing without bundled types");
                }
            }
        }
        Err(e) => {
            eprintln!("[Sidecar] Sidecar not ready: {}", e);
            eprintln!("[Sidecar] Skipping type resolution");
        }
    }
}

/// Discover files and extract symbols for MultiAgentOrchestrator.
/// Returns source files, symbols, function defs, repo name, and config/package.json paths
/// from a single directory traversal.
fn discover_files_and_symbols(repo_path: &str, cm: Lrc<SourceMap>) -> FileDiscoveryResult {
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));
    let repo_name = get_repository_name(repo_path);

    let ignore_patterns = ["node_modules", "dist", "build", ".next", "ts_check"];
    let (files, config_file, package_json) = find_files(repo_path, &ignore_patterns);

    println!(
        "Found {} files to analyze in directory {}",
        files.len(),
        repo_path
    );

    let mut all_imported_symbols = HashMap::new();
    let mut all_function_definitions = HashMap::new();

    for file_path in &files {
        if let Some(module) = parse_file(file_path, &cm, &handler) {
            let mut import_extractor = ImportSymbolExtractor::new();
            module.visit_with(&mut import_extractor);
            all_imported_symbols.extend(import_extractor.imported_symbols);

            let mut func_extractor = FunctionDefinitionExtractor::new(file_path.clone());
            module.visit_with(&mut func_extractor);
            all_function_definitions.extend(func_extractor.function_definitions);
        }
    }

    println!(
        "Extracted {} imported symbols and {} function definitions from {} files",
        all_imported_symbols.len(),
        all_function_definitions.len(),
        files.len()
    );

    Ok((
        files,
        all_imported_symbols,
        all_function_definitions,
        repo_name,
        config_file,
        package_json,
    ))
}

/// Load config and packages from pre-discovered file paths (avoids redundant directory traversal).
fn load_config_and_packages(
    config_file_path: Option<PathBuf>,
    package_json_path: Option<PathBuf>,
) -> Result<(Config, Packages), Box<dyn std::error::Error>> {
    let config = if let Some(config_path) = config_file_path {
        println!("Found carrick.json file: {}", config_path.display());
        Config::new(vec![config_path]).unwrap_or_else(|e| {
            eprintln!("Warning: Error parsing config file: {}", e);
            Config::default()
        })
    } else {
        Config::default()
    };

    let packages = if let Some(package_path) = package_json_path {
        println!("Found package.json: {}", package_path.display());
        Packages::new(vec![package_path]).unwrap_or_else(|e| {
            eprintln!("Warning: Error parsing package.json: {}", e);
            Packages::default()
        })
    } else {
        Packages::default()
    };

    Ok((config, packages))
}

fn build_type_manifest_entries(
    mount_graph: &MountGraph,
    config: &Config,
) -> Vec<TypeManifestEntry> {
    let normalizer = UrlNormalizer::new(config);
    let mut entries = Vec::new();

    for endpoint in mount_graph.get_resolved_endpoints() {
        let method = normalize_manifest_method(&endpoint.method);
        if !is_http_method(&method) {
            continue;
        }
        let path = endpoint.full_path.clone();
        if !path.starts_with('/') {
            continue;
        }
        let (file_path, line_number) = parse_file_location(&endpoint.file_location);

        add_manifest_pair(
            &mut entries,
            &method,
            &path,
            ManifestRole::Producer,
            &file_path,
            line_number,
            None,
        );
    }

    for call in mount_graph.get_data_calls() {
        if !normalizer.is_probable_url(&call.target_url) {
            continue;
        }
        let (file_path, line_number) = parse_file_location(&call.file_location);
        let method = normalize_manifest_method(&call.method);
        if !is_http_method(&method) {
            continue;
        }
        let path = normalizer.extract_path(&call.target_url);
        let call_id = build_call_site_id(&file_path, line_number, &method, &path);

        add_manifest_pair(
            &mut entries,
            &method,
            &path,
            ManifestRole::Consumer,
            &file_path,
            line_number,
            Some(&call_id),
        );
    }

    entries
}

fn add_manifest_pair(
    entries: &mut Vec<TypeManifestEntry>,
    method: &str,
    path: &str,
    role: ManifestRole,
    file_path: &str,
    line_number: u32,
    call_id: Option<&str>,
) {
    // Producers for GET/HEAD/OPTIONS never have request bodies
    let skip_request =
        role == ManifestRole::Producer && matches!(method, "GET" | "HEAD" | "OPTIONS");

    for type_kind in [ManifestTypeKind::Request, ManifestTypeKind::Response] {
        if skip_request && type_kind == ManifestTypeKind::Request {
            continue;
        }
        let type_alias =
            build_manifest_type_alias_with_call_id(method, path, role, type_kind, call_id);
        let infer_kind = infer_kind_for_manifest(role, type_kind);
        let evidence = crate::cloud_storage::TypeEvidence {
            file_path: file_path.to_string(),
            span_start: None,
            span_end: None,
            line_number,
            infer_kind,
            is_explicit: false,
            type_state: ManifestTypeState::Unknown,
        };
        entries.push(TypeManifestEntry {
            method: method.to_string(),
            path: path.to_string(),
            role,
            type_kind,
            type_alias,
            file_path: file_path.to_string(),
            line_number,
            is_explicit: false,
            type_state: ManifestTypeState::Unknown,
            evidence,
        });
    }
}

fn infer_kind_for_manifest(role: ManifestRole, type_kind: ManifestTypeKind) -> InferKind {
    match (role, type_kind) {
        (ManifestRole::Consumer, ManifestTypeKind::Response) => InferKind::CallResult,
        (_, ManifestTypeKind::Response) => InferKind::ResponseBody,
        (_, ManifestTypeKind::Request) => InferKind::RequestBody,
    }
}

/// Enrich manifest entries with type resolution results.
///
/// This function updates the `type_state` and `is_explicit` fields of manifest entries
/// based on the results from the TypeSidecar. Types that were successfully resolved
/// (either explicitly or through inference) will have their state updated from `Unknown`
/// to `Explicit` or `Implicit`.
fn enrich_manifest_with_type_resolution(
    manifest: &mut [TypeManifestEntry],
    type_resolution: &TypeResolutionResult,
    bundled_dts: Option<&str>,
) {
    // Build a lookup of resolved type aliases
    // Key: type_alias, Value: (type_string, is_explicit)
    let mut resolved_types: HashMap<String, (String, bool)> = HashMap::new();

    // Add explicit types from the manifest
    for entry in &type_resolution.explicit_manifest {
        resolved_types.insert(entry.alias.clone(), (entry.type_string.clone(), true));
    }

    // Add inferred types
    for inferred in &type_resolution.inferred_types {
        // Don't overwrite explicit types with inferred ones
        if !resolved_types.contains_key(&inferred.alias) {
            resolved_types.insert(
                inferred.alias.clone(),
                (inferred.type_string.clone(), inferred.is_explicit),
            );
        }
    }

    // Also check the bundled .d.ts content for defined types
    // This catches types that were successfully bundled but not in the manifest.
    // Exclude aliases defined as `= unknown` — those are placeholders for failed
    // inferences and should not be promoted to Implicit.
    let dts_defined_aliases: HashSet<String> = if let Some(dts) = bundled_dts {
        manifest
            .iter()
            .filter(|e| {
                dts_defines_alias(dts, &e.type_alias)
                    && !dts_alias_is_trivially_unknown(dts, &e.type_alias)
            })
            .map(|e| e.type_alias.clone())
            .collect()
    } else {
        HashSet::new()
    };

    // Update manifest entries
    for entry in manifest.iter_mut() {
        if let Some((type_string, is_explicit)) = resolved_types.get(&entry.type_alias) {
            // Check if the type is actually resolved (not "unknown")
            let is_unknown_type = type_string.trim() == "unknown"
                || type_string.trim() == "any"
                || type_string.is_empty();

            if !is_unknown_type {
                entry.is_explicit = *is_explicit;
                entry.type_state = if *is_explicit {
                    ManifestTypeState::Explicit
                } else {
                    ManifestTypeState::Implicit
                };
                entry.evidence.is_explicit = *is_explicit;
                entry.evidence.type_state = entry.type_state;
            }
        } else if dts_defined_aliases.contains(&entry.type_alias) {
            // Type is defined in the .d.ts but wasn't in our resolution results
            // This can happen for inline aliases or other edge cases
            entry.type_state = ManifestTypeState::Implicit;
            entry.evidence.type_state = ManifestTypeState::Implicit;
        }
    }

    // Log enrichment stats
    let explicit_count = manifest
        .iter()
        .filter(|e| e.type_state == ManifestTypeState::Explicit)
        .count();
    let implicit_count = manifest
        .iter()
        .filter(|e| e.type_state == ManifestTypeState::Implicit)
        .count();
    let unknown_count = manifest
        .iter()
        .filter(|e| e.type_state == ManifestTypeState::Unknown)
        .count();
    eprintln!(
        "[Manifest Enrichment] {} explicit, {} implicit, {} unknown",
        explicit_count, implicit_count, unknown_count
    );
}

#[derive(Serialize)]
struct TypeManifestFile {
    repo_name: String,
    commit_hash: String,
    entries: Vec<TypeManifestEntry>,
}

async fn analyze_current_repo(
    repo_path: &str,
    sidecar: Option<&TypeSidecar>,
) -> Result<CloudRepoData, Box<dyn std::error::Error>> {
    // Canonicalize repo_path for consistent path normalization between runs
    let canonical = std::fs::canonicalize(repo_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| repo_path.to_string());
    let repo_path = canonical.as_str();

    println!(
        "---> Running multi-agent framework-agnostic analysis on: {}",
        repo_path
    );

    let cm: Lrc<SourceMap> = Default::default();
    let (files, all_imported_symbols, function_definitions, repo_name, config_file, package_json) =
        discover_files_and_symbols(repo_path, cm.clone())?;
    println!(
        "Extracted repository name: '{}' from {} files ({} function definitions)",
        repo_name,
        files.len(),
        function_definitions.len()
    );

    let (config, packages) = load_config_and_packages(config_file, package_json)?;

    // 3. Get API key and create MultiAgentOrchestrator
    let api_key = env::var("CARRICK_API_KEY")
        .map_err(|_| "CARRICK_API_KEY environment variable must be set")?;
    let orchestrator = MultiAgentOrchestrator::new(api_key.clone(), cm.clone());

    // 4. Run the complete multi-agent analysis
    let analysis_result = orchestrator
        .run_complete_analysis(files, &packages, &all_imported_symbols)
        .await?;

    // 5. Build CloudRepoData directly from multi-agent results (bypassing Analyzer adapter layer)
    let mut cloud_data = CloudRepoData::from_multi_agent_results(
        repo_name.clone(),
        repo_path,
        &analysis_result,
        serde_json::to_string(&config).ok(),
        serde_json::to_string(&packages).ok(),
        Some(packages.clone()),
        function_definitions.clone(),
    );

    let manifest_entries = build_type_manifest_entries(&analysis_result.mount_graph, &config);
    if !manifest_entries.is_empty() {
        cloud_data.type_manifest = Some(manifest_entries);
    }

    // 6. Resolve types using sidecar if available
    let agent_service = AgentService::new(api_key);
    let file_orchestrator = FileOrchestrator::new(agent_service);

    resolve_types_if_available(
        sidecar,
        &file_orchestrator,
        &analysis_result.file_results,
        repo_path,
        &packages,
        &analysis_result.mount_graph,
        &config,
        &mut cloud_data,
    );

    if let Some(bundled_types) = cloud_data.bundled_types.take() {
        let updated = append_missing_aliases(bundled_types, cloud_data.type_manifest.as_ref());
        cloud_data.bundled_types = Some(updated);
    }

    // 7. Populate cache fields for future incremental runs
    let mut cached_file_results = analysis_result.file_results.clone();
    let normalized = normalize_file_results_keys(&cached_file_results, repo_path);
    cached_file_results = normalized;
    strip_diagnostic_fields(&mut cached_file_results);
    cloud_data.file_results = Some(cached_file_results);
    cloud_data.cached_detection = Some(analysis_result.framework_detection.clone());
    cloud_data.cached_guidance = Some(analysis_result.framework_guidance.clone());
    cloud_data.cache_version = Some(CACHE_VERSION);
    // Hash raw file content (not serialized struct) for deterministic comparison
    cloud_data.package_json_hash = std::fs::read_to_string(format!("{}/package.json", repo_path))
        .ok()
        .map(|content| hash_file_content(&content));

    Ok(cloud_data)
}

async fn build_cross_repo_analyzer(
    mut all_repo_data: Vec<CloudRepoData>,
    current_repo_data: CloudRepoData,
) -> Result<Analyzer, Box<dyn std::error::Error>> {
    // Add current repo data to the mix
    all_repo_data.push(current_repo_data);
    // 1. Merge configs and packages using generic function
    let combined_config = merge_serialized_data(&all_repo_data, |data| data.config_json.as_ref())?;
    let combined_packages =
        merge_serialized_data(&all_repo_data, |data| data.package_json.as_ref())?;

    // 2. Build analyzer using shared logic (skip type resolution for cross-repo)
    let cm: Lrc<SourceMap> = Default::default();
    let builder = AnalyzerBuilder::new_for_cross_repo(combined_config, cm);
    let mut analyzer = builder.build_from_repo_data(all_repo_data.clone()).await?;

    // 3. Merge mount graphs from all repos for framework-agnostic analysis
    let merged_mount_graph = MountGraph::merge_from_repos(&all_repo_data);
    analyzer.set_mount_graph(merged_mount_graph);

    // 4. Add packages data from all repos for dependency analysis
    for repo_data in &all_repo_data {
        if let Some(packages) = &repo_data.packages {
            analyzer.add_repo_packages(repo_data.repo_name.clone(), packages.clone());
        }
    }

    // 5. Recreate type files from S3 and run type checking
    recreate_type_files_and_check(&all_repo_data, &combined_packages)?;

    // 6. Run final type checking
    if let Err(e) = analyzer.run_final_type_checking() {
        println!("⚠️  Warning: Type checking failed: {}", e);
    }

    Ok(analyzer)
}

fn recreate_type_files_and_check(
    all_repo_data: &[CloudRepoData],
    packages: &Packages,
) -> Result<(), Box<dyn std::error::Error>> {
    let output_dir = std::path::Path::new("ts_check/output");
    if output_dir.exists() {
        println!("Cleaning output directory: ts_check/output");
        if let Err(e) = std::fs::remove_dir_all(output_dir) {
            println!("Warning: Failed to clean output directory: {}", e);
        }
    }

    if let Err(e) = std::fs::create_dir_all(output_dir) {
        println!("Warning: Failed to create output directory: {}", e);
    } else {
        println!("Created clean output directory: ts_check/output");
    }

    for repo_data in all_repo_data {
        if let Some(bundled_types) = &repo_data.bundled_types {
            let safe_repo_name = repo_data.repo_name.replace("/", "_");
            let file_name = format!("{}_types.d.ts", safe_repo_name);
            let file_path = output_dir.join(&file_name);
            let content =
                append_missing_aliases(bundled_types.clone(), repo_data.type_manifest.as_ref());

            if let Err(e) = std::fs::write(&file_path, content) {
                println!("Warning: Failed to write type file {}: {}", file_name, e);
            } else {
                println!("Created bundled type file: {}", file_path.display());
            }
        } else {
            println!(
                "No bundled types available for repo: {}",
                repo_data.repo_name
            );
        }
    }

    write_manifest_files(all_repo_data, output_dir)?;

    // Recreate package.json and tsconfig.json after writing type files
    recreate_package_and_tsconfig(output_dir, packages)?;

    Ok(())
}

fn write_manifest_files(
    all_repo_data: &[CloudRepoData],
    output_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut producer_entries = Vec::new();
    let mut consumer_entries = Vec::new();

    for repo_data in all_repo_data {
        if let Some(entries) = &repo_data.type_manifest {
            for entry in entries {
                match entry.role {
                    ManifestRole::Producer => producer_entries.push(entry.clone()),
                    ManifestRole::Consumer => consumer_entries.push(entry.clone()),
                }
            }
        }
    }

    let producer_manifest = TypeManifestFile {
        repo_name: "cross-repo-producers".to_string(),
        commit_hash: "mixed".to_string(),
        entries: producer_entries,
    };
    let consumer_manifest = TypeManifestFile {
        repo_name: "cross-repo-consumers".to_string(),
        commit_hash: "mixed".to_string(),
        entries: consumer_entries,
    };

    let producer_path = output_dir.join("producer-manifest.json");
    let consumer_path = output_dir.join("consumer-manifest.json");

    std::fs::write(
        &producer_path,
        serde_json::to_string_pretty(&producer_manifest)?,
    )?;
    std::fs::write(
        &consumer_path,
        serde_json::to_string_pretty(&consumer_manifest)?,
    )?;

    println!(
        "Wrote manifest files: {} ({} entries), {} ({} entries)",
        producer_path.display(),
        producer_manifest.entries.len(),
        consumer_path.display(),
        consumer_manifest.entries.len()
    );

    Ok(())
}

fn append_missing_aliases(content: String, manifest: Option<&Vec<TypeManifestEntry>>) -> String {
    let Some(entries) = manifest else {
        return content;
    };

    let mut updated = content;
    let mut seen = std::collections::HashSet::new();

    for entry in entries {
        if !seen.insert(entry.type_alias.clone()) {
            continue;
        }

        if dts_defines_alias(&updated, &entry.type_alias) {
            continue;
        }

        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str("export type ");
        updated.push_str(&entry.type_alias);
        updated.push_str(" = unknown;\n");
    }

    updated
}

fn dts_defines_alias(content: &str, alias: &str) -> bool {
    let escaped = regex::escape(alias);
    let pattern = format!(r"\b(type|interface|class|enum|namespace)\s+{}\b", escaped);
    match regex::Regex::new(&pattern) {
        Ok(re) => re.is_match(content),
        Err(_) => false,
    }
}

/// Returns true when the .d.ts defines the alias as exactly `= unknown`,
/// i.e. it's a placeholder for a failed inference, not a real type.
fn dts_alias_is_trivially_unknown(content: &str, alias: &str) -> bool {
    let escaped = regex::escape(alias);
    let pattern = format!(r"type\s+{}\s*=\s*unknown\s*;", escaped);
    match regex::Regex::new(&pattern) {
        Ok(re) => re.is_match(content),
        Err(_) => false,
    }
}

/// Recreate package.json and tsconfig.json in the output directory
fn recreate_package_and_tsconfig(
    output_dir: &std::path::Path,
    packages: &Packages,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create package.json
    let package_json_path = output_dir.join("package.json");
    let package_dependencies = packages.get_dependencies();

    // Convert PackageInfo objects to simple version strings for npm
    let mut dependencies = std::collections::HashMap::new();
    for (name, package_info) in package_dependencies {
        dependencies.insert(name.clone(), package_info.version.clone());
    }

    // Only add essential TypeScript dependencies if they're missing
    if !dependencies.contains_key("typescript") {
        dependencies.insert("typescript".to_string(), "5.8.3".to_string());
    }
    if !dependencies.contains_key("ts-node") {
        dependencies.insert("ts-node".to_string(), "10.9.2".to_string());
    }

    let package_json_content = serde_json::json!({
        "name": "carrick-type-check",
        "version": "1.0.0",
        "dependencies": dependencies
    });

    std::fs::write(
        &package_json_path,
        serde_json::to_string_pretty(&package_json_content)?,
    )?;
    println!("Recreated package.json at {}", package_json_path.display());

    let skip_npm_install = std::env::var("CARRICK_SKIP_NPM_INSTALL").is_ok()
        || std::env::var("CARRICK_MOCK_ALL").is_ok();

    if skip_npm_install {
        println!("Skipping npm install (mock mode or CARRICK_SKIP_NPM_INSTALL set)");
    } else {
        // Clean any existing node_modules and package-lock.json to avoid conflicts
        let node_modules_path = output_dir.join("node_modules");
        let package_lock_path = output_dir.join("package-lock.json");

        if node_modules_path.exists() {
            println!("Removing existing node_modules directory...");
            std::fs::remove_dir_all(&node_modules_path).ok();
        }

        if package_lock_path.exists() {
            println!("Removing existing package-lock.json...");
            std::fs::remove_file(&package_lock_path).ok();
        }

        // Install dependencies
        use std::process::Command;
        println!("Installing dependencies...");

        let install_output = Command::new("npm")
            .arg("install")
            .current_dir(output_dir)
            .output()
            .map_err(|e| format!("Failed to run npm install: {}", e))?;

        if !install_output.status.success() {
            let stderr = String::from_utf8_lossy(&install_output.stderr);
            eprintln!("Warning: npm install failed: {}", stderr);
        } else {
            println!("Dependencies installed successfully");
        }
    }

    // Create tsconfig.json with dynamic path mappings based on actual type files
    let tsconfig_path = output_dir.join("tsconfig.json");
    let tsconfig_content = create_dynamic_tsconfig(output_dir);

    std::fs::write(
        &tsconfig_path,
        serde_json::to_string_pretty(&tsconfig_content)?,
    )?;
    println!("Recreated tsconfig.json at {}", tsconfig_path.display());

    Ok(())
}

fn print_results(result: crate::analyzer::ApiAnalysisResult) {
    let formatted_output = crate::formatter::FormattedOutput::new(result);
    formatted_output.print();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::ApiEndpointDetails;
    use crate::visitor::{OwnerType, TypeReference};
    use std::path::PathBuf;

    #[test]
    fn test_ast_stripping_removes_nodes() {
        // Create test CloudRepoData with AST nodes
        let endpoint = ApiEndpointDetails {
            owner: Some(OwnerType::App("test_app".to_string())),
            route: "/test".to_string(),
            method: "GET".to_string(),
            params: vec![],
            request_body: None,
            response_body: None,
            file_path: PathBuf::from("test.js"),
            request_type: Some(TypeReference {
                file_path: PathBuf::from("test.ts"),
                type_ann: None,
                start_position: 0,
                composite_type_string: "TestType".to_string(),
                alias: "TestType".to_string(),
            }),
            response_type: Some(TypeReference {
                file_path: PathBuf::from("test.ts"),
                type_ann: None,
                start_position: 0,
                composite_type_string: "ResponseType".to_string(),
                alias: "ResponseType".to_string(),
            }),
            handler_name: Some("testHandler".to_string()),
        };

        let test_data = CloudRepoData {
            repo_name: "test-repo".to_string(),
            service_name: None,
            package_name: None,
            endpoints: vec![endpoint.clone()],
            calls: vec![endpoint.clone()],
            mounts: vec![],
            apps: std::collections::HashMap::new(),
            imported_handlers: vec![],
            function_definitions: std::collections::HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            package_json_hash: None,
            cache_version: None,
        };

        // Verify strip_ast_nodes removes AST nodes
        let stripped = strip_ast_nodes(test_data);

        assert!(stripped.endpoints[0].request_type.is_none());
        assert!(stripped.endpoints[0].response_type.is_none());
        assert!(stripped.calls[0].request_type.is_none());
        assert!(stripped.calls[0].response_type.is_none());
    }

    #[test]
    fn test_merge_serialized_data() {
        use crate::config::Config;
        use crate::packages::Packages;

        let test_data = vec![CloudRepoData {
            repo_name: "test-repo".to_string(),
            service_name: None,
            package_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: std::collections::HashMap::new(),
            imported_handlers: vec![],
            function_definitions: std::collections::HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            package_json_hash: None,
            cache_version: None,
        }];

        // Test Config merging
        let merged_config: Result<Config, _> =
            merge_serialized_data(&test_data, |data| data.config_json.as_ref());
        assert!(merged_config.is_ok());

        // Test Packages merging
        let merged_packages: Result<Packages, _> =
            merge_serialized_data(&test_data, |data| data.package_json.as_ref());
        assert!(merged_packages.is_ok());

        // Test with empty data returns default
        let empty_data: Vec<CloudRepoData> = vec![];
        let default_config: Result<Config, _> =
            merge_serialized_data(&empty_data, |data| data.config_json.as_ref());
        assert!(default_config.is_ok());
    }

    #[tokio::test]
    async fn test_cross_repo_analyzer_builder_no_sourcemap_issues() {
        use crate::analyzer::builder::AnalyzerBuilder;
        use crate::config::Config;
        use swc_common::{SourceMap, sync::Lrc};

        // Create test data with TypeReferences that would cause SourceMap issues
        let endpoint = ApiEndpointDetails {
            owner: Some(OwnerType::App("test_app".to_string())),
            route: "/test".to_string(),
            method: "GET".to_string(),
            params: vec![],
            request_body: None,
            response_body: None,
            file_path: PathBuf::from("test.js"),
            request_type: Some(TypeReference {
                file_path: PathBuf::from("test.ts"),
                type_ann: None,
                start_position: 999999, // This would cause SourceMap issues
                composite_type_string: "TestType".to_string(),
                alias: "TestType".to_string(),
            }),
            response_type: Some(TypeReference {
                file_path: PathBuf::from("test.ts"),
                type_ann: None,
                start_position: 999999, // This would cause SourceMap issues
                composite_type_string: "ResponseType".to_string(),
                alias: "ResponseType".to_string(),
            }),
            handler_name: Some("testHandler".to_string()),
        };

        let test_data = vec![CloudRepoData {
            repo_name: "test-repo".to_string(),
            service_name: None,
            package_name: None,
            endpoints: vec![endpoint.clone()],
            calls: vec![endpoint.clone()],
            mounts: vec![],
            apps: std::collections::HashMap::new(),
            imported_handlers: vec![],
            function_definitions: std::collections::HashMap::new(),
            config_json: Some(r#"{"ignore_patterns": [], "type_check": false}"#.to_string()),
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: None,
            cached_detection: None,
            cached_guidance: None,
            package_json_hash: None,
            cache_version: None,
        }];

        // Test that cross-repo builder doesn't fail with SourceMap issues
        let cm: Lrc<SourceMap> = Default::default();
        let config = Config::default();
        let builder = AnalyzerBuilder::new_for_cross_repo(config, cm);

        // This should not panic with SourceMap issues
        let result = builder.build_from_repo_data(test_data).await;
        assert!(result.is_ok(), "build_cross_repo_analyzer should not fail");

        let analyzer = result.unwrap();
        assert_eq!(analyzer.endpoints.len(), 1);
        assert_eq!(analyzer.calls.len(), 1);
    }

    // === Incremental analysis tests ===

    use crate::agents::file_analyzer_agent::{
        DataCallResult, EndpointResult, FileAnalysisResult, MountResult,
    };

    fn make_file_result(endpoints: Vec<&str>, data_calls: Vec<&str>) -> FileAnalysisResult {
        FileAnalysisResult {
            mounts: vec![],
            endpoints: endpoints
                .into_iter()
                .map(|path| EndpointResult {
                    candidate_id: "cand_123".to_string(),
                    line_number: 10,
                    owner_node: "app".to_string(),
                    method: "GET".to_string(),
                    path: path.to_string(),
                    handler_name: "handler".to_string(),
                    pattern_matched: "app.get(...)".to_string(),
                    call_expression_span_start: Some(100),
                    call_expression_span_end: Some(200),
                    payload_expression_text: Some("req.body".to_string()),
                    payload_expression_line: Some(11),
                    response_expression_text: Some("res.json(data)".to_string()),
                    response_expression_line: Some(12),
                    primary_type_symbol: None,
                    type_import_source: None,
                })
                .collect(),
            data_calls: data_calls
                .into_iter()
                .map(|target| DataCallResult {
                    candidate_id: "cand_456".to_string(),
                    line_number: 20,
                    target: target.to_string(),
                    method: Some("GET".to_string()),
                    pattern_matched: "fetch(...)".to_string(),
                    call_expression_span_start: Some(300),
                    call_expression_span_end: Some(400),
                    call_expression_text: Some("fetch('/api')".to_string()),
                    call_expression_line: Some(21),
                    payload_expression_text: Some("body".to_string()),
                    payload_expression_line: Some(22),
                    primary_type_symbol: None,
                    type_import_source: None,
                })
                .collect(),
        }
    }

    #[test]
    fn test_hash_file_content_deterministic() {
        let hash1 = hash_file_content("hello world");
        let hash2 = hash_file_content("hello world");
        assert_eq!(hash1, hash2);

        let hash3 = hash_file_content("different content");
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_normalize_file_results_keys_absolute_path() {
        let mut results = HashMap::new();
        results.insert(
            "/home/user/repo/src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec![]),
        );
        results.insert(
            "/home/user/repo/src/routes.ts".to_string(),
            make_file_result(vec!["/api/posts"], vec![]),
        );

        let normalized = normalize_file_results_keys(&results, "/home/user/repo");

        assert!(normalized.contains_key("src/app.ts"));
        assert!(normalized.contains_key("src/routes.ts"));
        assert_eq!(normalized.len(), 2);
    }

    #[test]
    fn test_normalize_file_results_keys_dot_prefix() {
        let mut results = HashMap::new();
        results.insert(
            "./src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec![]),
        );

        let normalized = normalize_file_results_keys(&results, ".");

        assert!(normalized.contains_key("src/app.ts"));
    }

    #[test]
    fn test_normalize_file_results_keys_already_relative() {
        let mut results = HashMap::new();
        results.insert(
            "src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec![]),
        );

        let normalized = normalize_file_results_keys(&results, "/some/other/path");

        // Key doesn't match prefix, should be kept as-is
        assert!(normalized.contains_key("src/app.ts"));
    }

    #[test]
    fn test_strip_diagnostic_fields() {
        let mut results = HashMap::new();
        results.insert(
            "src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec!["/api/posts"]),
        );

        // Add a mount with pattern_matched
        results
            .get_mut("src/app.ts")
            .unwrap()
            .mounts
            .push(MountResult {
                line_number: 5,
                parent_node: "app".to_string(),
                child_node: "router".to_string(),
                mount_path: "/api".to_string(),
                import_source: Some("./routes".to_string()),
                pattern_matched: "app.use('/api', router)".to_string(),
            });

        strip_diagnostic_fields(&mut results);

        let result = &results["src/app.ts"];

        // Endpoint diagnostic fields should be cleared
        assert_eq!(result.endpoints[0].candidate_id, "");
        assert_eq!(result.endpoints[0].pattern_matched, "");
        assert!(result.endpoints[0].payload_expression_text.is_none());
        assert!(result.endpoints[0].response_expression_text.is_none());
        // Non-diagnostic fields should be preserved
        assert_eq!(result.endpoints[0].path, "/api/users");
        assert_eq!(result.endpoints[0].method, "GET");
        assert_eq!(result.endpoints[0].handler_name, "handler");
        assert_eq!(result.endpoints[0].line_number, 10);

        // Data call diagnostic fields should be cleared
        assert_eq!(result.data_calls[0].candidate_id, "");
        assert_eq!(result.data_calls[0].pattern_matched, "");
        assert!(result.data_calls[0].call_expression_text.is_none());
        assert!(result.data_calls[0].payload_expression_text.is_none());
        // Non-diagnostic fields preserved
        assert_eq!(result.data_calls[0].target, "/api/posts");

        // Mount diagnostic fields should be cleared
        assert_eq!(result.mounts[0].pattern_matched, "");
        // Non-diagnostic fields preserved
        assert_eq!(result.mounts[0].mount_path, "/api");
        assert_eq!(result.mounts[0].parent_node, "app");
    }

    #[test]
    fn test_get_changed_files_with_real_git_repo() {
        use std::process::Command;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let repo_path = temp_dir.path().to_str().unwrap();

        // Init a git repo
        Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        // Create initial commit with a .ts file
        std::fs::write(temp_dir.path().join("app.ts"), "const x = 1;").unwrap();
        std::fs::write(temp_dir.path().join("readme.md"), "# Readme").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        // Get the first commit hash
        let base_hash = String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo_path)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        // Make changes: modify .ts, add new .tsx, modify .md (should be filtered)
        std::fs::write(temp_dir.path().join("app.ts"), "const x = 2;").unwrap();
        std::fs::write(
            temp_dir.path().join("new.tsx"),
            "export default () => <div/>;",
        )
        .unwrap();
        std::fs::write(temp_dir.path().join("readme.md"), "# Updated").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "changes"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        // Test get_changed_files
        let changed = get_changed_files(repo_path, &base_hash);
        assert!(changed.is_some());

        let changed = changed.unwrap();
        assert!(changed.contains(&"app.ts".to_string()));
        assert!(changed.contains(&"new.tsx".to_string()));
        // .md file should be filtered out
        assert!(!changed.contains(&"readme.md".to_string()));
    }

    #[test]
    fn test_get_changed_files_returns_none_for_invalid_commit() {
        use std::process::Command;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let repo_path = temp_dir.path().to_str().unwrap();

        Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        std::fs::write(temp_dir.path().join("app.ts"), "x").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(repo_path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(repo_path)
            .output()
            .unwrap();

        // Non-existent commit hash → should return None (simulates shallow clone)
        let result = get_changed_files(repo_path, "0000000000000000000000000000000000000000");
        assert!(result.is_none());
    }

    #[test]
    fn test_payload_size_guard_drops_file_results_when_too_large() {
        // Create CloudRepoData with large file_results
        let mut large_results = HashMap::new();
        // Create entries large enough to exceed 5MB
        for i in 0..1000 {
            let large_string = "x".repeat(5000);
            large_results.insert(
                format!("src/file_{}.ts", i),
                FileAnalysisResult {
                    mounts: vec![],
                    endpoints: vec![EndpointResult {
                        candidate_id: large_string.clone(),
                        line_number: 1,
                        owner_node: "app".to_string(),
                        method: "GET".to_string(),
                        path: large_string.clone(),
                        handler_name: large_string.clone(),
                        pattern_matched: large_string.clone(),
                        call_expression_span_start: None,
                        call_expression_span_end: None,
                        payload_expression_text: Some(large_string.clone()),
                        payload_expression_line: None,
                        response_expression_text: Some(large_string),
                        response_expression_line: None,
                        primary_type_symbol: None,
                        type_import_source: None,
                    }],
                    data_calls: vec![],
                },
            );
        }

        let data = CloudRepoData {
            repo_name: "test-repo".to_string(),
            service_name: None,
            package_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: HashMap::new(),
            imported_handlers: vec![],
            function_definitions: HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: Some(large_results),
            cached_detection: None,
            cached_guidance: None,
            package_json_hash: None,
            cache_version: Some(CACHE_VERSION),
        };

        let stripped = strip_ast_nodes(data);

        // file_results should be dropped because payload exceeds 5MB
        assert!(
            stripped.file_results.is_none(),
            "file_results should be dropped when payload exceeds 5MB"
        );
    }

    #[test]
    fn test_payload_size_guard_keeps_small_file_results() {
        let mut small_results = HashMap::new();
        small_results.insert(
            "src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec![]),
        );

        let data = CloudRepoData {
            repo_name: "test-repo".to_string(),
            service_name: None,
            package_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: HashMap::new(),
            imported_handlers: vec![],
            function_definitions: HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "test-hash".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: Some(small_results),
            cached_detection: None,
            cached_guidance: None,
            package_json_hash: None,
            cache_version: Some(CACHE_VERSION),
        };

        let stripped = strip_ast_nodes(data);

        // file_results should be preserved (small payload)
        assert!(
            stripped.file_results.is_some(),
            "file_results should be preserved when payload is small"
        );
    }

    #[test]
    fn test_file_results_merge_handles_deletes_and_additions() {
        // Simulate: previous run had files A, B, C
        // Current run discovers A, B, D (C deleted, D new)
        // Git diff says B changed
        let mut prev_results = HashMap::new();
        prev_results.insert(
            "src/a.ts".to_string(),
            make_file_result(vec!["/api/a"], vec![]),
        );
        prev_results.insert(
            "src/b.ts".to_string(),
            make_file_result(vec!["/api/b"], vec![]),
        );
        prev_results.insert(
            "src/c.ts".to_string(),
            make_file_result(vec!["/api/c"], vec![]),
        );

        // Current file discovery
        let current_file_set: HashSet<String> = ["src/a.ts", "src/b.ts", "src/d.ts"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let changed_set: HashSet<String> = ["src/b.ts"].iter().map(|s| s.to_string()).collect();

        // New results from analyzing changed + new files
        let mut new_results = HashMap::new();
        new_results.insert(
            "src/b.ts".to_string(),
            make_file_result(vec!["/api/b_v2"], vec![]),
        );
        new_results.insert(
            "src/d.ts".to_string(),
            make_file_result(vec!["/api/d"], vec![]),
        );

        // Merge logic (mirrors analyze_current_repo_incremental)
        let mut merged: HashMap<String, FileAnalysisResult> = HashMap::new();

        // Copy cached results for unchanged files that still exist
        for (path, result) in &prev_results {
            if current_file_set.contains(path) && !changed_set.contains(path) {
                merged.insert(path.clone(), result.clone());
            }
        }

        // Insert new/changed results
        for (path, result) in new_results {
            merged.insert(path, result);
        }

        // Verify merge result
        assert_eq!(
            merged.len(),
            3,
            "Should have A (cached), B (fresh), D (new)"
        );
        assert!(merged.contains_key("src/a.ts"), "A should be cached");
        assert!(merged.contains_key("src/b.ts"), "B should be fresh");
        assert!(merged.contains_key("src/d.ts"), "D should be new");
        assert!(!merged.contains_key("src/c.ts"), "C should be deleted");

        // A should have old data
        assert_eq!(merged["src/a.ts"].endpoints[0].path, "/api/a");
        // B should have new data
        assert_eq!(merged["src/b.ts"].endpoints[0].path, "/api/b_v2");
        // D should have new data
        assert_eq!(merged["src/d.ts"].endpoints[0].path, "/api/d");
    }

    #[test]
    fn test_file_results_serialization_roundtrip() {
        // Verify FileAnalysisResult survives JSON serialization (critical for AWS cache)
        let result = make_file_result(vec!["/api/users", "/api/posts"], vec!["/external/api"]);

        let json = serde_json::to_string(&result).expect("should serialize");
        let deserialized: FileAnalysisResult =
            serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(deserialized.endpoints.len(), 2);
        assert_eq!(deserialized.data_calls.len(), 1);
        assert_eq!(deserialized.endpoints[0].path, "/api/users");
        assert_eq!(deserialized.data_calls[0].target, "/external/api");
    }

    #[test]
    fn test_cloud_repo_data_with_file_results_roundtrip() {
        // Verify CloudRepoData with file_results survives JSON roundtrip
        let mut file_results = HashMap::new();
        file_results.insert(
            "src/app.ts".to_string(),
            make_file_result(vec!["/api/users"], vec![]),
        );

        let data = CloudRepoData {
            repo_name: "test-repo".to_string(),
            service_name: None,
            package_name: None,
            endpoints: vec![],
            calls: vec![],
            mounts: vec![],
            apps: HashMap::new(),
            imported_handlers: vec![],
            function_definitions: HashMap::new(),
            config_json: None,
            package_json: None,
            packages: None,
            last_updated: chrono::Utc::now(),
            commit_hash: "abc123".to_string(),
            mount_graph: None,
            bundled_types: None,
            type_manifest: None,
            file_results: Some(file_results),
            cached_detection: Some(DetectionResult {
                frameworks: vec!["express".to_string()],
                data_fetchers: vec!["fetch".to_string()],
                notes: "test".to_string(),
            }),
            cached_guidance: None,
            package_json_hash: Some("abc123hash".to_string()),
            cache_version: Some(CACHE_VERSION),
        };

        let json = serde_json::to_string(&data).expect("should serialize");
        let deserialized: CloudRepoData = serde_json::from_str(&json).expect("should deserialize");

        assert!(deserialized.file_results.is_some());
        let fr = deserialized.file_results.unwrap();
        assert!(fr.contains_key("src/app.ts"));
        assert_eq!(fr["src/app.ts"].endpoints[0].path, "/api/users");

        assert!(deserialized.cached_detection.is_some());
        assert_eq!(
            deserialized.cached_detection.unwrap().frameworks,
            vec!["express"]
        );
        assert_eq!(deserialized.cache_version, Some(CACHE_VERSION));
        assert_eq!(
            deserialized.package_json_hash,
            Some("abc123hash".to_string())
        );
    }

    #[test]
    fn test_cloud_repo_data_without_cache_fields_deserializes() {
        // Old CloudRepoData without cache fields should still deserialize (backwards compat)
        let json = r#"{
            "repo_name": "old-repo",
            "endpoints": [],
            "calls": [],
            "mounts": [],
            "apps": {},
            "imported_handlers": [],
            "function_definitions": {},
            "last_updated": "2025-01-01T00:00:00Z",
            "commit_hash": "old123"
        }"#;

        let data: CloudRepoData =
            serde_json::from_str(json).expect("should deserialize old format");
        assert_eq!(data.repo_name, "old-repo");
        assert!(data.file_results.is_none());
        assert!(data.cached_detection.is_none());
        assert!(data.cached_guidance.is_none());
        assert!(data.package_json_hash.is_none());
        assert!(data.cache_version.is_none());
    }
}
