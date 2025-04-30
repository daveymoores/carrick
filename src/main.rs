mod analyzer;
mod config;
mod extractor;
mod file_finder;
mod parser;
mod router_context;
mod visitor;
use analyzer::analyze_api_consistency;
use config::Config;
use file_finder::find_files;
use parser::parse_file;
use swc_common::{
    SourceMap,
    errors::{ColorConfig, Handler},
    sync::Lrc,
};
use swc_ecma_visit::VisitWith;
use visitor::DependencyVisitor;

fn main() {
    // Create shared source map and error handler
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));
    let mut configs = Vec::new();

    // Extract directories from args if they exist. If no args are given then default to the current directory.
    let repositories = std::env::args();
    let repo_dirs = match repositories.len() == 0 {
        true => vec![".".to_string()],
        false => repositories.collect(),
    };

    // Create Visitors vec for each file in every repo
    let mut visitors = Vec::new();
    for dir in repo_dirs {
        println!("---> Analyzing JavaScript/TypeScript files in: {}", dir);

        // Files to ignore - if possible use existing tooling to build this list
        let ignore_patterns = ["node_modules", "dist", "build", ".next"];

        // Find all JS/TS files and the config file
        let (files, config_file_path) = find_files(&dir, &ignore_patterns);
        println!(
            "Found {} files to analyze in directory {}",
            files.len(),
            &dir
        );

        // Process the config file if found
        if let Some(config_path) = config_file_path {
            println!("Found configuration file: {}", config_path.display());
            configs.push(config_path);
        }

        // Process each JS/TS file
        for file_path in files {
            println!("Parsing: {}", file_path.display());
            if let Some(module) = parse_file(&file_path, &cm, &handler) {
                let mut visitor = DependencyVisitor::new(file_path.clone());
                module.visit_with(&mut visitor);
                visitors.push(visitor);
            }
        }
    }

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

    // Analyze for inconsistencies
    let result = analyze_api_consistency(visitors, config);

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
