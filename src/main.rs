mod analyzer;
mod extractor;
mod file_finder;
mod parser;
mod visitor;
use analyzer::analyze_api_consistency;
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

    // Directory to analyze
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    println!("Analyzing JavaScript/TypeScript files in: {}", dir);

    // Files to ignore
    let ignore_patterns = ["node_modules", "dist", "build", ".next"];

    // Find all JS/TS files
    let files = find_files(&dir, &ignore_patterns);
    println!("Found {} files to analyze", files.len());

    // Process each file
    let mut visitors = Vec::new();
    for file_path in files {
        println!("Parsing: {}", file_path.display());
        if let Some(module) = parse_file(&file_path, &cm, &handler) {
            // Create visitor with file path
            let mut visitor = DependencyVisitor::new(file_path.clone());
            module.visit_with(&mut visitor);
            visitors.push(visitor);
        }
    }

    // Analyze for inconsistencies
    let result = analyze_api_consistency(visitors);

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
        let mismatches = result.issues.mismatches;
        let mut issue_number: usize = 0;

        for (i, issue) in call_issues.iter().enumerate() {
            issue_number = i + 1;
            println!("\n{}. {}", &issue_number, issue);
        }

        for (_, issue) in endpoint_issues.iter().enumerate() {
            issue_number = issue_number + 1;
            print!("\n{}. {}", &issue_number, issue);
        }

        for (_, issue) in mismatches.iter().enumerate() {
            issue_number = issue_number + 1;
            print!("\n{}. {}", &issue_number, issue);
        }
    }
}
