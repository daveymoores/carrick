mod analyzer;
mod app_context;
mod config;
mod extractor;
mod file_finder;
mod packages;
mod parser;
mod router_context;
mod utils;
mod visitor;
use analyzer::analyze_api_consistency;
use config::Config;
use file_finder::find_files;
use packages::Packages;
use parser::parse_file;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;
use visitor::DependencyVisitor;

/// Resolves a relative import path to an absolute file path
fn resolve_import_path(base_file: &Path, import_path: &str) -> Option<PathBuf> {
    if import_path.starts_with('.') {
        // It's a relative import
        let base_dir = base_file.parent()?;

        // Remove leading "./" or "../" but keep the path structure
        let normalized_path = if import_path.starts_with("./") {
            &import_path[2..]
        } else if import_path.starts_with("../") {
            let mut dir_path = base_dir.to_path_buf();
            dir_path.pop(); // Go up one directory for ../
            return resolve_import_path(&dir_path.join("dummy.js"), &import_path[3..]);
        } else {
            import_path
        };
        // Try different extensions and index files
        let extensions = ["", ".js", ".ts", ".jsx", ".tsx"];
        let index_extensions = ["/index.js", "/index.ts", "/index.jsx", "/index.tsx"];

        for ext in &extensions {
            let full_path = base_dir.join(format!("{}{}", normalized_path, ext));
            if full_path.exists() {
                return Some(full_path);
            }
        }

        // Try as directory with index file
        for index_ext in &index_extensions {
            let full_path = base_dir.join(format!("{}{}", normalized_path, index_ext));
            println!("{:?}", full_path);
            if full_path.exists() {
                return Some(full_path);
            }
        }

        // If we couldn't find the file with extensions, return the base path anyway
        // The file might be in a different format or require more complex resolution
        Some(base_dir.join(normalized_path))
    } else {
        // Non-relative imports (e.g., 'express', 'cors') - not local files
        None
    }
}

fn main() {
    // Create shared source map and error handler
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));
    let mut configs = Vec::new();
    let mut package_jsons = Vec::new();

    // Extract directories from args if they exist. If no args are given then default to the current directory.
    let repositories = std::env::args().skip(1); // Skip program name
    let repo_dirs = if repositories.len() == 0 {
        vec![".".to_string()]
    } else {
        repositories.collect()
    };

    // Track processed files to avoid duplicates
    let mut processed_file_paths = HashSet::new(); // HashSet<String>

    // Queue to store files for processing [file_path, repo_prefix]
    let mut file_queue = VecDeque::new();

    // Find all files to process initially and queue them
    for dir in &repo_dirs {
        println!("---> Analyzing JavaScript/TypeScript files in: {}", dir);

        let dir_paths: Vec<_> = dir.split("/").filter(|s| !s.is_empty()).collect();
        let repo_prefix = dir_paths.last().unwrap_or(&"default").to_string();

        let ignore_patterns = ["node_modules", "dist", "build", ".next"];
        let (files, config_file_path, package_json_path) = find_files(&dir, &ignore_patterns);

        println!(
            "Found {} files to analyze in directory {}",
            files.len(),
            &dir
        );

        // Process the config file if found
        if let Some(package_json) = package_json_path {
            println!("Found package.json: {}", package_json.display());
            package_jsons.push(package_json);
        }

        // Process the config file if found
        if let Some(config_path) = config_file_path {
            println!("Found carrick.json file: {}", config_path.display());
            configs.push(config_path);
        }

        // Queue all discovered files for processing
        for file_path in files {
            file_queue.push_back((file_path, repo_prefix.clone(), None));
        }
    }

    // Process all files in the queue (including newly discovered imports)
    let mut visitors = Vec::new();

    while let Some((file_path, repo_prefix, imported_router_name)) = file_queue.pop_front() {
        let path_str = file_path.to_string_lossy().to_string();
        if processed_file_paths.contains(&path_str) {
            continue;
        }
        processed_file_paths.insert(path_str.clone());

        println!("Parsing: {}", file_path.display());

        if let Some(module) = parse_file(&file_path, &cm, &handler) {
            // Create visitor with the imported router name if this file was imported as a router
            let mut visitor =
                DependencyVisitor::new(file_path.clone(), &repo_prefix, imported_router_name, cm.clone());
            module.visit_with(&mut visitor);

            // Queue imported router files that might be used with app.use or router.use
            for (name, symbol) in &visitor.imported_symbols {
                // Check if this import is used as a router in a mount
                let is_router = visitor.mounts.iter().any(|mount| {
                    match &mount.child {
                        visitor::OwnerType::Router(router_name) => {
                            // Extract just the local name without the repo prefix
                            let parts: Vec<_> = router_name.split(':').collect();
                            let local_name = parts.last().unwrap_or(&"");
                            local_name == name
                        }
                        _ => false,
                    }
                });

                if is_router {
                    println!("Following import '{}' from '{}'", name, symbol.source);

                    // Try to resolve the relative import path
                    if let Some(resolved_path) = resolve_import_path(&file_path, &symbol.source) {
                        println!("Resolved to: {}", resolved_path.display());

                        // Queue for processing with the imported router name
                        // This will be used by the visitor to correctly name the router
                        file_queue.push_back((
                            resolved_path,
                            repo_prefix.clone(),
                            Some(name.clone()),
                        ));
                    }
                }
            }

            // Store the visitor for later path resolution
            visitors.push(visitor);
        }
    }

    // Load config
    let config = match Config::new(configs) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Error parsing config file: {}", error);
            eprintln!(
                "Please ensure your carrick.json file is valid JSON and follows the expected format."
            );
            std::process::exit(1);
        }
    };

    // Load packages
    let packages = match Packages::new(package_jsons) {
        Ok(packages) => packages,
        Err(error) => {
            eprintln!("Error parsing package.json files: {}", error);
            eprintln!("Please ensure your package.json files are valid JSON.");
            std::process::exit(1);
        }
    };

    // Analyze for inconsistencies. Pass the sourcemap to allow relative byte positions to be calculated
    let result = analyze_api_consistency(visitors, config, packages, cm, repo_dirs);

    // Print results
    println!("\nAPI Analysis Results:");
    println!("=====================");
    println!(
        "Found {} endpoints across all files",
        result.endpoints.len()
    );
    println!("Found {} API calls across all files", result.calls.len());

    if result.issues.is_empty() {
        println!("\n✅  No API inconsistencies detected!");
    } else {
        println!("\n⚠️  Found {} API issues:", result.issues.len());
        let call_issues = result.issues.call_issues;
        let endpoint_issues = result.issues.endpoint_issues;
        let env_var_calls = result.issues.env_var_calls;
        let mismatches = result.issues.mismatches;
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

        if !env_var_calls.is_empty() {
            for issue in env_var_calls.iter() {
                issue_number = issue_number + 1;
                print!(
                    "\n{}. {}\n     • Consider adding to known external APIs configuration",
                    &issue_number, issue
                );
            }
        }
    }
}
