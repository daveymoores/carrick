use crate::analyzer::{Analyzer, ApiEndpointDetails, builder::AnalyzerBuilder};
use crate::cloud_storage::{CloudRepoData, CloudStorage, get_current_commit_hash};
use crate::config::{Config, create_dynamic_tsconfig};
use crate::file_finder::find_files;
use crate::packages::Packages;
use crate::parser::parse_file;
use crate::utils::{get_repository_name, resolve_import_path};
use crate::visitor::DependencyVisitor;
use chrono::Utc;
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;

use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;

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

pub async fn run_analysis_engine<T: CloudStorage>(
    storage: T,
    repo_path: &str,
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

    // 1. Analyze current repo only
    let current_repo_data = analyze_current_repo(repo_path).await?;
    println!("Analyzed current repo: {}", current_repo_data.repo_name);

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
    let (mut all_repo_data, repo_s3_urls) = storage // Updated to destructure tuple
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
    let analyzer =
        build_cross_repo_analyzer(all_repo_data, current_repo_data, repo_s3_urls, &storage).await?;
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

/// Extract file discovery and parsing logic from analyze_current_repo
fn discover_and_parse_files(
    repo_path: &str,
    cm: Lrc<SourceMap>,
) -> Result<(Vec<DependencyVisitor>, String), Box<dyn std::error::Error>> {
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

    // Track processed files to avoid duplicates
    let mut processed_file_paths = HashSet::new();
    let mut file_queue = VecDeque::new();

    // Queue all discovered files for processing
    for file_path in files {
        file_queue.push_back((file_path, repo_name.clone(), None));
    }

    // Process all files in the queue (including newly discovered imports)
    let mut visitors = Vec::new();

    while let Some((file_path, repo_prefix, imported_router_name)) = file_queue.pop_front() {
        let path_str = file_path.to_string_lossy().to_string();
        let processing_key = match &imported_router_name {
            Some(name) => format!("{}#{}", path_str, name),
            None => path_str.clone(),
        };
        if processed_file_paths.contains(&processing_key) {
            continue;
        }
        processed_file_paths.insert(processing_key);

        println!("Parsing: {}", file_path.display());

        if let Some(module) = parse_file(&file_path, &cm, &handler) {
            let mut visitor = DependencyVisitor::new(
                file_path.clone(),
                &repo_prefix,
                imported_router_name,
                cm.clone(),
            );
            module.visit_with(&mut visitor);

            // Queue imported router files that might be used with app.use or router.use
            for (name, symbol) in &visitor.imported_symbols {
                let is_router = visitor.mounts.iter().any(|mount| match &mount.child {
                    crate::visitor::OwnerType::Router(router_name) => {
                        let parts: Vec<_> = router_name.split(':').collect();
                        let local_name = parts.last().unwrap_or(&"");
                        local_name == name
                    }
                    _ => false,
                });

                if is_router {
                    println!("Following import '{}' from '{}'", name, symbol.source);

                    if let Some(resolved_path) = resolve_import_path(&file_path, &symbol.source) {
                        println!("Resolved to: {}", resolved_path.display());
                        file_queue.push_back((
                            resolved_path,
                            repo_prefix.clone(),
                            Some(name.clone()),
                        ));
                    }
                }
            }

            visitors.push(visitor);
        }
    }

    Ok((visitors, repo_name))
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

async fn analyze_current_repo(
    repo_path: &str,
) -> Result<CloudRepoData, Box<dyn std::error::Error>> {
    println!(
        "---> Analyzing JavaScript/TypeScript files in: {}",
        repo_path
    );

    // 1. Create shared SourceMap for consistent byte position tracking
    let cm: Lrc<SourceMap> = Default::default();

    // 2. Discover and parse files using shared SourceMap
    let (visitors, repo_name) = discover_and_parse_files(repo_path, cm.clone())?;
    println!("Extracted repository name: '{}'", repo_name);

    // 3. Load config and packages
    let (config, packages) = load_config_and_packages(repo_path)?;

    // 4. Build analyzer using shared logic with same SourceMap
    let builder = AnalyzerBuilder::new(config.clone(), cm);
    let analyzer = builder.build_from_visitors(visitors).await?;

    // 5. Extract types for current repo
    extract_types_for_current_repo(&analyzer, repo_path, &packages)?;

    // 6. Build CloudRepoData (AST stripping handled by caller)
    let cloud_data = CloudRepoData {
        repo_name: repo_name.clone(),
        endpoints: analyzer.endpoints,
        calls: analyzer.calls,
        mounts: analyzer.mounts,
        apps: analyzer.apps,
        imported_handlers: analyzer.imported_handlers,
        function_definitions: analyzer.function_definitions,
        config_json: serde_json::to_string(&config).ok(),
        package_json: serde_json::to_string(&packages).ok(),
        packages: Some(packages),
        last_updated: Utc::now(),
        commit_hash: get_current_commit_hash(),
    };

    Ok(cloud_data)
}

fn extract_types_for_current_repo(
    analyzer: &Analyzer,
    repo_path: &str,
    packages: &Packages,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashMap;
    let mut repo_type_map: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    let repo_paths = vec![repo_path.to_string()];

    // Group type information by repository using endpoint owner information
    for endpoint in &analyzer.endpoints {
        let repo_prefix = analyzer.extract_repo_prefix_from_owner(&endpoint.owner);

        analyzer.process_api_detail_types(endpoint, repo_prefix, &mut repo_type_map);
    }

    // Group type information by repository using call file information
    for call in &analyzer.calls {
        let repo_prefix = analyzer.extract_repo_prefix_from_file_path(&call.file_path, &repo_paths);
        analyzer.process_api_detail_types(call, repo_prefix, &mut repo_type_map);
    }

    // Collect type information from Gemini-extracted fetch_calls
    let gemini_type_infos = analyzer.collect_type_infos_from_calls(analyzer.fetch_calls());
    for type_info in gemini_type_infos {
        let file_path = type_info["filePath"].as_str().unwrap_or("");
        let repo_prefix = analyzer
            .extract_repo_prefix_from_file_path(&std::path::PathBuf::from(file_path), &repo_paths);
        repo_type_map
            .entry(repo_prefix)
            .or_default()
            .push(type_info);
    }

    // Extract types for current repository
    let repo_name = get_repository_name(repo_path);

    let type_infos = repo_type_map.get(&repo_name).cloned().unwrap_or_default();

    if !type_infos.is_empty() {
        println!(
            "Processing {} types from repository: {}",
            type_infos.len(),
            repo_path
        );
        analyzer.extract_types_for_repo(repo_path, type_infos.clone(), packages);
    }

    Ok(())
}

async fn build_cross_repo_analyzer<T: CloudStorage>(
    mut all_repo_data: Vec<CloudRepoData>,
    current_repo_data: CloudRepoData,
    repo_s3_urls: HashMap<String, String>,
    storage: &T,
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

    // 3. Add packages data from all repos for dependency analysis
    for repo_data in &all_repo_data {
        if let Some(packages) = &repo_data.packages {
            analyzer.add_repo_packages(repo_data.repo_name.clone(), packages.clone());
        }
    }

    // 4. Recreate type files from S3 and run type checking
    recreate_type_files_and_check(&all_repo_data, &repo_s3_urls, storage, &combined_packages)
        .await?;

    // 5. Run final type checking
    if let Err(e) = analyzer.run_final_type_checking() {
        println!("⚠️  Warning: Type checking failed: {}", e);
    }

    Ok(analyzer)
}

async fn recreate_type_files_and_check<T: CloudStorage>(
    all_repo_data: &[CloudRepoData],
    repo_s3_urls: &HashMap<String, String>, // Map repo_name -> s3_url
    storage: &T,
    packages: &Packages,
) -> Result<(), Box<dyn std::error::Error>> {
    // Before cleaning output directory, copy current repo type file to temp
    let current_repo = all_repo_data.last().unwrap().repo_name.replace("/", "_");
    let generated_type_file = format!("ts_check/output/{}_types.ts", current_repo);
    let temp_dir = std::path::Path::new("ts_check/temp");
    if !temp_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(temp_dir) {
            println!("Warning: Failed to create temp directory: {}", e);
        }
    }
    let temp_type_file = temp_dir.join(format!("{}_types.ts", current_repo));
    if std::fs::copy(&generated_type_file, &temp_type_file).is_ok() {
        println!(
            "Backed up type file before cleaning: {}",
            temp_type_file.display()
        );
    } else {
        println!(
            "Warning: Could not backup type file before cleaning: {}",
            generated_type_file
        );
    }

    // Clean output directory
    let output_dir = std::path::Path::new("ts_check/output");
    if output_dir.exists() {
        println!("Cleaning output directory: ts_check/output");
        if let Err(e) = std::fs::remove_dir_all(output_dir) {
            println!("Warning: Failed to clean output directory: {}", e);
        }
    }

    // Create clean output directory
    if let Err(e) = std::fs::create_dir_all(output_dir) {
        println!("Warning: Failed to create output directory: {}", e);
    } else {
        println!("Created clean output directory: ts_check/output");
    }

    // Debug: Print the full repo_s3_urls map before download
    println!("repo_s3_urls map before download: {:?}", repo_s3_urls);

    // Download type files for each repository
    for repo_data in all_repo_data {
        println!(
            "Attempting to download type file for repo: {}",
            repo_data.repo_name
        );
        // Use local type file for current repo, download from S3 for others
        if repo_data.repo_name == all_repo_data.last().unwrap().repo_name {
            // Assume last in all_repo_data is current repo (matches how current_repo_data is appended)
            let safe_repo_name = repo_data.repo_name.replace("/", "_");
            let file_name = format!("{}_types.ts", safe_repo_name);
            let file_path = output_dir.join(&file_name);
            // Move the backed up type file from temp into output directory
            let temp_type_file = format!("ts_check/temp/{}_types.ts", safe_repo_name);
            match std::fs::copy(&temp_type_file, &file_path) {
                Ok(_) => println!(
                    "Moved type file from temp for current repo: {}",
                    file_path.display()
                ),
                Err(e) => println!(
                    "Warning: Failed to move type file from temp {}: {}",
                    temp_type_file, e
                ),
            }
            // Clean up temp directory after moving the file
            if let Err(e) = std::fs::remove_dir_all("ts_check/temp") {
                println!("Warning: Failed to clean temp directory: {}", e);
            }
        } else if let Some(s3_url) = repo_s3_urls.get(&repo_data.repo_name) {
            println!(
                "Downloading type file for repository: {}",
                repo_data.repo_name
            );

            match storage.download_type_file_content(s3_url).await {
                Ok(type_content) => {
                    // Create a safe filename from repo name
                    let safe_repo_name = repo_data.repo_name.replace("/", "_");
                    let file_name = format!("{}_types.ts", safe_repo_name);
                    let file_path = output_dir.join(&file_name);

                    if let Err(e) = std::fs::write(&file_path, type_content) {
                        println!("Warning: Failed to write type file {}: {}", file_name, e);
                    } else {
                        println!("Created type file: {}", file_path.display());
                    }
                }
                Err(e) => {
                    println!(
                        "Warning: Failed to download type file for repo {}: {}",
                        repo_data.repo_name, e
                    );
                }
            }
        } else {
            println!("No S3 URL found for repository: {}", repo_data.repo_name);
            println!(
                "repo_s3_urls keys: {:?}",
                repo_s3_urls.keys().collect::<Vec<_>>()
            );
        }
    }

    // Recreate package.json and tsconfig.json after downloading type files
    recreate_package_and_tsconfig(output_dir, packages)?;

    Ok(())
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
