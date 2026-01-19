use crate::analyzer::{Analyzer, ApiEndpointDetails, builder::AnalyzerBuilder};
use crate::cloud_storage::{
    CloudRepoData, CloudStorage, ManifestRole, ManifestTypeKind, ManifestTypeState,
    TypeManifestEntry,
};
use crate::config::{Config, create_dynamic_tsconfig};
use crate::file_finder::find_files;
use crate::mount_graph::MountGraph;
use crate::multi_agent_orchestrator::MultiAgentOrchestrator;
use crate::packages::Packages;
use crate::parser::parse_file;
use crate::services::{TypeSidecar, type_sidecar::InferKind};
use crate::type_manifest::{
    build_call_site_id, build_manifest_type_alias_with_call_id, is_http_method,
    normalize_manifest_method, parse_file_location,
};
use crate::url_normalizer::UrlNormalizer;
use crate::utils::get_repository_name;
use crate::visitor::{FunctionDefinition, FunctionDefinitionExtractor, ImportSymbolExtractor};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;

// Type aliases to reduce complexity
type FileDiscoveryResult = Result<
    (
        Vec<PathBuf>,
        HashMap<String, crate::visitor::ImportedSymbol>,
        HashMap<String, FunctionDefinition>,
        String,
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
    // TODO we have no way of finding unique org names, so I think this will need to be a token
    let carrick_org = env::var("CARRICK_ORG").map_err(|_| "CARRICK_ORG must be set in CI mode")?;

    // Determine if we should upload based on branch/event type
    let should_upload = should_upload_data();
    println!(
        "Running Carrick in CI mode with org: {} (upload: {})",
        &carrick_org, should_upload
    );

    storage
        .health_check()
        .await
        .map_err(|e| format!("Failed to connect to AWS services: {}", e))?;
    println!("AWS connectivity verified");

    // 1. Analyze current repo only (with optional sidecar for type resolution)
    let current_repo_data = analyze_current_repo(repo_path, sidecar).await?;
    println!("Analyzed current repo: {}", current_repo_data.repo_name);

    // Log sidecar type resolution results if available
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

    // 2. Conditionally upload current repo data to cloud storage
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

    // 3. Download data from all repos
    let (mut all_repo_data, _repo_s3_urls) = storage // Updated to destructure tuple
        .download_all_repo_data(&carrick_org)
        .await
        .map_err(|e| format!("Failed to download cross-repo data: {}", e))?;

    // Remove current repo from cross-repo data to prevent duplicate processing
    let current_repo_name = &current_repo_data.repo_name;
    all_repo_data.retain(|repo| &repo.repo_name != current_repo_name);

    println!(
        "Downloaded data from {} repos (excluding current repo: {})",
        all_repo_data.len(),
        current_repo_name
    );

    // 4. Reconstruct analyzer with combined data (including current repo)
    let analyzer = build_cross_repo_analyzer(all_repo_data, current_repo_data).await?;
    println!("Reconstructed analyzer with cross-repo data");

    // 5. Run analysis
    let results = analyzer.get_results();

    // 6. Print results
    print_results(results);

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
        use std::path::PathBuf;
        let mut temp_files = Vec::new();

        // Write each config to a temporary file
        for (i, repo_data) in all_repo_data.iter().enumerate() {
            if let Some(json_str) = extractor(repo_data) {
                let temp_path = PathBuf::from(format!("/tmp/carrick_config_{}.json", i));
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

/// Remove AST nodes from CloudRepoData for serialization
fn strip_ast_nodes(mut data: CloudRepoData) -> CloudRepoData {
    fn strip_endpoint_ast(endpoint: &mut ApiEndpointDetails) {
        endpoint.request_type = None;
        endpoint.response_type = None;
    }

    data.endpoints.iter_mut().for_each(strip_endpoint_ast);
    data.calls.iter_mut().for_each(strip_endpoint_ast);
    data
}

/// Find the generated TypeScript file for the repo (heuristic: look for ts_check/output/*.ts)
/// Discover files and extract symbols for MultiAgentOrchestrator
fn discover_files_and_symbols(repo_path: &str, cm: Lrc<SourceMap>) -> FileDiscoveryResult {
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));
    let repo_name = get_repository_name(repo_path);

    // Find files in current repo only
    let ignore_patterns = ["node_modules", "dist", "build", ".next", "ts_check"];
    let (files, _, _) = find_files(repo_path, &ignore_patterns);

    println!(
        "Found {} files to analyze in directory {}",
        files.len(),
        repo_path
    );

    // Extract imported symbols and function definitions by parsing files
    let mut all_imported_symbols = HashMap::new();
    let mut all_function_definitions = HashMap::new();

    for file_path in &files {
        if let Some(module) = parse_file(file_path, &cm, &handler) {
            // Extract import symbols
            let mut import_extractor = ImportSymbolExtractor::new();
            module.visit_with(&mut import_extractor);
            all_imported_symbols.extend(import_extractor.imported_symbols);

            // Extract function definitions with type annotations
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
    ))
}

/// Extract config and package loading logic
fn load_config_and_packages(
    repo_path: &str,
) -> Result<(Config, Packages), Box<dyn std::error::Error>> {
    let ignore_patterns = ["node_modules", "dist", "build", ".next", "ts_check"];
    let (_, config_file_path, package_json_path) = find_files(repo_path, &ignore_patterns);

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
        let path = endpoint.full_path.clone();
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
    for type_kind in [ManifestTypeKind::Request, ManifestTypeKind::Response] {
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
    println!(
        "---> Running multi-agent framework-agnostic analysis on: {}",
        repo_path
    );

    // 1. Create shared SourceMap and discover files and symbols
    let cm: Lrc<SourceMap> = Default::default();
    let (files, all_imported_symbols, function_definitions, repo_name) =
        discover_files_and_symbols(repo_path, cm.clone())?;
    println!(
        "Extracted repository name: '{}' from {} files ({} function definitions)",
        repo_name,
        files.len(),
        function_definitions.len()
    );

    // 2. Load config and packages using existing logic
    let (config, packages) = load_config_and_packages(repo_path)?;

    // 3. Get API key and create MultiAgentOrchestrator
    let api_key = env::var("CARRICK_API_KEY")
        .map_err(|_| "CARRICK_API_KEY environment variable must be set")?;
    let orchestrator = MultiAgentOrchestrator::new(api_key, cm.clone());

    // 4. Run the complete multi-agent analysis
    let analysis_result = orchestrator
        .run_complete_analysis(files, &packages, &all_imported_symbols)
        .await?;

    // 5. Build CloudRepoData directly from multi-agent results (bypassing Analyzer adapter layer)
    let mut cloud_data = CloudRepoData::from_multi_agent_results(
        repo_name.clone(),
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

    // 6. Resolve types using sidecar if available (Phase 2.4)
    if let Some(sidecar) = sidecar {
        println!("\n=== Sidecar Type Resolution (Phase 2.4) ===");

        // Wait for sidecar to be ready (it should be by now, but ensure it)
        match sidecar.wait_ready(Duration::from_secs(10)) {
            Ok(()) => {
                // Create FileOrchestrator to use its type resolution method
                let api_key = env::var("CARRICK_API_KEY").unwrap_or_default();
                let agent_service = crate::agent_service::AgentService::new(api_key);
                let file_orchestrator =
                    crate::agents::file_orchestrator::FileOrchestrator::new(agent_service);

                // Resolve types using sidecar
                match file_orchestrator.resolve_types_with_sidecar(
                    sidecar,
                    &analysis_result.file_results,
                    repo_path,
                    &packages,
                    &analysis_result.mount_graph,
                    &config,
                ) {
                    Ok(type_resolution) => {
                        println!(
                            "Type resolution successful: {} explicit, {} inferred, {} failures",
                            type_resolution.explicit_manifest.len(),
                            type_resolution.inferred_types.len(),
                            type_resolution.symbol_failures.len()
                        );

                        // Phase 2.5: Populate CloudRepoData with bundled types
                        cloud_data.bundled_types = type_resolution.dts_content.clone();

                        // Log any failures for debugging
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

    if let Some(bundled_types) = cloud_data.bundled_types.take() {
        let updated = append_missing_aliases(bundled_types, cloud_data.type_manifest.as_ref());
        cloud_data.bundled_types = Some(updated);
    }

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
}
