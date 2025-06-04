use crate::analyzer::{Analyzer, ApiEndpointDetails};
use crate::cloud_storage::{CloudRepoData, CloudStorage, get_current_commit_hash};
use crate::config::Config;
use crate::file_finder::find_files;
use crate::packages::Packages;
use crate::parser::parse_file;
use crate::resolve_import_path;
use crate::visitor::DependencyVisitor;
use chrono::Utc;
use std::collections::{HashSet, VecDeque};
use std::env;
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;

pub async fn run_ci_mode<T: CloudStorage>(
    storage: T,
    repo_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let carrick_token =
        env::var("CARRICK_TOKEN").map_err(|_| "CARRICK_TOKEN must be set in CI mode")?;

    println!("Running Carrick in CI mode with token: {}", &carrick_token);

    // Verify MongoDB connectivity early
    storage
        .health_check()
        .await
        .map_err(|e| format!("Failed to connect to MongoDB: {}", e))?;
    println!("MongoDB connection verified");

    // 1. Analyze current repo only
    let current_repo_data = analyze_current_repo(repo_path)?;
    println!("Analyzed current repo: {}", current_repo_data.repo_name);

    // 2. Upload current repo data to cloud storage (without AST nodes)
    let cloud_data_serialized = serialize_cloud_repo_data_without_ast(&current_repo_data);
    storage
        .upload_repo_data(&carrick_token, &cloud_data_serialized)
        .await
        .map_err(|e| format!("Failed to upload repo data: {}", e))?;
    println!("Uploaded current repo data to cloud storage");

    // 2b. Upload generated TypeScript file to MongoDB
    if let Some(ts_file_path) = find_generated_typescript_file(repo_path) {
        match std::fs::read_to_string(&ts_file_path) {
            Ok(ts_content) => {
                storage
                    .upload_type_file(
                        &carrick_token,
                        &cloud_data_serialized.repo_name,
                        &ts_file_path,
                        &ts_content,
                    )
                    .await
                    .map_err(|e| format!("Failed to upload TypeScript file: {}", e))?;
                println!("Uploaded generated TypeScript file to MongoDB");
            }
            Err(e) => {
                println!(
                    "DEBUG: WARNING - Failed to read TypeScript file {}: {}",
                    ts_file_path, e
                );
            }
        }
    }

    // 3. Download data from all repos with same token
    let all_repo_data = storage
        .download_all_repo_data(&carrick_token)
        .await
        .map_err(|e| format!("Failed to download cross-repo data: {}", e))?;
    println!("Downloaded data from {} repos", all_repo_data.len());

    // 4. Reconstruct analyzer with combined data
    let analyzer = build_cross_repo_analyzer(all_repo_data)?;
    println!("Reconstructed analyzer with cross-repo data");

    // 5. Run analysis (same logic as local mode)
    let results = analyzer.get_results();

    // 6. Print results (same as local mode)
    print_results(results);

    Ok(())
}

/// Serialize CloudRepoData without AST nodes in ApiEndpointDetails
fn serialize_cloud_repo_data_without_ast(data: &CloudRepoData) -> CloudRepoData {
    CloudRepoData {
        repo_name: data.repo_name.clone(),
        endpoints: strip_ast_from_endpoints(data.endpoints.clone()),
        calls: strip_ast_from_endpoints(data.calls.clone()),
        mounts: data.mounts.clone(),
        apps: data.apps.clone(),
        imported_handlers: data.imported_handlers.clone(),
        function_definitions: data.function_definitions.clone(),
        config_json: data.config_json.clone(),
        package_json: data.package_json.clone(),
        extracted_types: data.extracted_types.clone(),
        last_updated: data.last_updated,
        commit_hash: data.commit_hash.clone(),
    }
}

fn strip_ast_from_endpoints(endpoints: Vec<ApiEndpointDetails>) -> Vec<ApiEndpointDetails> {
    fn strip_ast_from_endpoint(endpoint: &ApiEndpointDetails) -> ApiEndpointDetails {
        ApiEndpointDetails {
            owner: endpoint.owner.clone(),
            route: endpoint.route.clone(),
            method: endpoint.method.clone(),
            params: endpoint.params.clone(),
            request_body: endpoint.request_body.clone(),
            response_body: endpoint.response_body.clone(),
            file_path: endpoint.file_path.clone(),
            // Strip AST nodes - set to None for serialization
            request_type: None,
            response_type: None,
            handler_name: endpoint.handler_name.clone(),
        }
    }

    endpoints.iter().map(strip_ast_from_endpoint).collect()
}

/// Find the generated TypeScript file for the repo (heuristic: look for ts_check/output/*.ts)
fn find_generated_typescript_file(_repo_path: &str) -> Option<String> {
    use std::fs;
    use std::path::Path;

    let output_dir = Path::new("ts_check/output");
    if output_dir.exists() {
        if let Ok(entries) = fs::read_dir(output_dir) {
            let all_entries: Vec<_> = entries.flatten().collect();

            for entry in all_entries {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext == "ts" {
                        return Some(path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    None
}

fn analyze_current_repo(repo_path: &str) -> Result<CloudRepoData, Box<dyn std::error::Error>> {
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

    println!(
        "---> Analyzing JavaScript/TypeScript files in: {}",
        repo_path
    );

    let repo_name = repo_path
        .split("/")
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(".")
        .to_string();

    // Find files in current repo only
    let ignore_patterns = ["node_modules", "dist", "build", ".next"];
    let (files, config_file_path, package_json_path) = find_files(repo_path, &ignore_patterns);

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
        // Create a unique key that includes the imported router name to allow
        // the same file to be processed multiple times with different contexts
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

    // Create analyzer and extract data
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

    let config_clone = config.clone();
    let packages_clone = packages.clone();
    let mut analyzer = Analyzer::new(config, cm.clone());

    // Add visitor data to analyzer
    for visitor in visitors {
        analyzer.add_visitor_data(visitor);
    }

    // Resolve endpoint paths and types (this populates request_type and response_type fields)
    let endpoints =
        analyzer.resolve_all_endpoint_paths(&analyzer.endpoints, &analyzer.mounts, &analyzer.apps);
    analyzer.endpoints = endpoints;
    analyzer.resolve_types_for_endpoints(cm.clone());

    // Extract type information for current repo (now that types are resolved)
    let extracted_types = extract_types_for_current_repo(&analyzer, repo_path, &packages_clone)?;

    // Build CloudRepoData (strip AST information for serialization)
    let cloud_data = CloudRepoData {
        repo_name: repo_name.clone(),
        endpoints: strip_ast_from_endpoints(analyzer.endpoints.clone()),
        calls: strip_ast_from_endpoints(analyzer.calls.clone()),
        mounts: analyzer.mounts.clone(),
        apps: analyzer.apps.clone(),
        imported_handlers: analyzer.imported_handlers.clone(),
        function_definitions: analyzer.function_definitions.clone(),
        config_json: serde_json::to_string(&config_clone).ok(),
        package_json: serde_json::to_string(&packages_clone).ok(),
        extracted_types,
        last_updated: Utc::now(),
        commit_hash: get_current_commit_hash(),
    };

    Ok(cloud_data)
}

fn extract_types_for_current_repo(
    analyzer: &Analyzer,
    repo_path: &str,
    packages: &Packages,
) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
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

    // Extract types for current repository
    let repo_name = repo_path
        .split("/")
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or("default");

    let type_infos = repo_type_map.get(repo_name).cloned().unwrap_or_default();

    if !type_infos.is_empty() {
        println!(
            "Processing {} types from repository: {}",
            type_infos.len(),
            repo_path
        );
        analyzer.extract_types_for_repo(repo_path, type_infos.clone(), packages);
    }

    Ok(type_infos)
}

fn build_cross_repo_analyzer(
    all_repo_data: Vec<CloudRepoData>,
) -> Result<Analyzer, Box<dyn std::error::Error>> {
    // Combine all configs
    let combined_config = merge_configs(&all_repo_data)?;

    // Combine all packages
    let combined_packages = merge_packages(&all_repo_data)?;

    // Create analyzer with combined config
    let cm: Lrc<SourceMap> = Default::default();
    let mut analyzer = Analyzer::new(combined_config, cm.clone());

    // Populate analyzer with data from all repos
    for repo_data in &all_repo_data {
        analyzer.endpoints.extend(repo_data.endpoints.clone());
        analyzer.calls.extend(repo_data.calls.clone());
        analyzer.mounts.extend(repo_data.mounts.clone());
        analyzer.apps.extend(repo_data.apps.clone());
        analyzer
            .imported_handlers
            .extend(repo_data.imported_handlers.clone());
        analyzer
            .function_definitions
            .extend(repo_data.function_definitions.clone());
    }

    // Resolve endpoint paths (same as local mode)
    let endpoints = analyzer.resolve_all_endpoint_paths(
        &analyzer.endpoints.clone(),
        &analyzer.mounts.clone(),
        &analyzer.apps.clone(),
    );
    analyzer.endpoints = endpoints;

    // Build router
    analyzer.build_endpoint_router();

    // Resolve types and perform analysis
    let (response_fields, request_fields) = analyzer.resolve_imported_handler_route_fields(
        &analyzer.imported_handlers.clone(),
        &analyzer.function_definitions.clone(),
    );

    analyzer
        .update_endpoints_with_resolved_fields(response_fields, request_fields)
        .resolve_types_for_endpoints(cm.clone())
        .analyze_functions_for_fetch_calls();

    // Recreate type files from stored data and run type checking
    recreate_type_files_and_check(&all_repo_data, &combined_packages)?;

    // Run final type checking
    if let Err(e) = analyzer.run_final_type_checking() {
        println!("⚠️  Warning: Type checking failed: {}", e);
    }

    Ok(analyzer)
}

fn merge_configs(all_repo_data: &[CloudRepoData]) -> Result<Config, Box<dyn std::error::Error>> {
    // For now, just use the first available config
    // TODO: Implement proper config merging logic
    for repo_data in all_repo_data {
        if let Some(config_json) = &repo_data.config_json {
            if let Ok(config) = serde_json::from_str::<Config>(config_json) {
                return Ok(config);
            }
        }
    }
    Ok(Config::default())
}

fn merge_packages(all_repo_data: &[CloudRepoData]) -> Result<Packages, Box<dyn std::error::Error>> {
    // For now, just use the first available packages
    // TODO: Implement proper package merging logic
    for repo_data in all_repo_data {
        if let Some(package_json) = &repo_data.package_json {
            if let Ok(packages) = serde_json::from_str::<Packages>(package_json) {
                return Ok(packages);
            }
        }
    }
    Ok(Packages::default())
}

fn recreate_type_files_and_check(
    all_repo_data: &[CloudRepoData],
    packages: &Packages,
) -> Result<(), Box<dyn std::error::Error>> {
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

    // Recreate type files for each repository
    for repo_data in all_repo_data {
        if !repo_data.extracted_types.is_empty() {
            println!(
                "Recreating {} type files for repository: {}",
                repo_data.extracted_types.len(),
                repo_data.repo_name
            );

            // Create a temporary analyzer to use the extract_types_for_repo method
            let cm: Lrc<SourceMap> = Default::default();
            let temp_analyzer = Analyzer::new(Config::default(), cm);

            // Use the repo name as the "path" since we don't have actual file paths in CI
            temp_analyzer.extract_types_for_repo(
                &repo_data.repo_name,
                repo_data.extracted_types.clone(),
                packages,
            );
        }
    }

    Ok(())
}

fn print_results(result: crate::analyzer::ApiAnalysisResult) {
    println!("\nAPI Analysis Results:");
    println!("=====================");
    println!(
        "Found {} endpoints across all files",
        result.endpoints.len()
    );
    println!("Found {} API calls across all files", result.calls.len());

    if result.issues.is_empty() {
        println!("\nNo API inconsistencies detected!");
    } else {
        println!("\nFound {} API issues:", result.issues.len());
        let call_issues = result.issues.call_issues;
        let endpoint_issues = result.issues.endpoint_issues;
        let env_var_calls = result.issues.env_var_calls;
        let mismatches = result.issues.mismatches;
        let type_mismatches = result.issues.type_mismatches;
        let mut issue_number: usize = 0;

        if !call_issues.is_empty() {
            for (i, issue) in call_issues.iter().enumerate() {
                issue_number = i + 1;
                print!("\n{}. {}", &issue_number, issue);
            }
        }

        if !endpoint_issues.is_empty() {
            for issue in endpoint_issues.iter() {
                issue_number = issue_number + 1;
                print!("\n{}. {}", &issue_number, issue);
            }
        }

        for issue in mismatches.iter() {
            issue_number = issue_number + 1;
            print!("\n{}. {}", &issue_number, issue);
        }

        if !type_mismatches.is_empty() {
            for issue in type_mismatches.iter() {
                issue_number = issue_number + 1;
                print!("\n{}. {}", &issue_number, issue);
            }
        }

        if !env_var_calls.is_empty() {
            for issue in env_var_calls.iter() {
                issue_number = issue_number + 1;
                print!(
                    "\n{}. {}\n     - Consider adding to known external APIs configuration",
                    &issue_number, issue
                );
            }
        }
    }
}
