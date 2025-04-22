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
        println!("\n>>>>>>>>>>>>><<<<<<<<<<<<<");
        println!("Parsing: {}", file_path.display());
        println!(">>>>>>>>>>>>><<<<<<<<<<<<<\n");
        if let Some(module) = parse_file(&file_path, &cm, &handler) {
            // Create visitor with file path
            let mut visitor = DependencyVisitor::new(file_path.clone());
            module.visit_with(&mut visitor);
            visitor.print_imported_handler_summary();
            visitor.print_function_definitions();
            visitors.push(visitor);
        }
    }

    // Print overall imported handler summary
    println!("\n=== Overall Imported Handler Summary ===");
    let total_imported_handlers: usize = visitors.iter().map(|v| v.imported_handlers.len()).sum();
    println!("Total imported handlers used: {}", total_imported_handlers);

    // You could also gather all imported handlers into one collection
    let all_imported_handlers: Vec<_> = visitors
        .iter()
        .flat_map(|v| v.imported_handlers.clone())
        .collect();

    println!("\nAll imported handlers:");
    for (route, handler, source) in &all_imported_handlers {
        println!("Route: {} uses handler {} from {}", route, handler, source);
    }

    // Print overall function definitions summary
    println!("\n=== Overall Function Definitions ===");
    let total_functions: usize = visitors.iter().map(|v| v.function_definitions.len()).sum();
    println!("Total function definitions found: {}", total_functions);

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
        println!("\n✅ No API inconsistencies detected!");
    } else {
        println!("\n⚠️ Found {} API issues:", result.issues.len());
        for (i, issue) in result.issues.iter().enumerate() {
            println!("{}. {}", i + 1, issue);
        }
    }
}
