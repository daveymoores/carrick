mod analyzer;
mod app_context;
mod call_site_classifier;
mod call_site_extractor;
mod cloud_storage;
mod config;
mod engine;
mod extractor;
mod file_finder;
mod formatter;
mod framework_detector;
mod gemini_service;
mod mount_graph;
mod multi_agent_orchestrator;
mod packages;
mod parser;
mod router_context;
mod utils;
mod visitor;

use crate::cloud_storage::{AwsStorage, MockStorage};
use engine::run_analysis_engine;

#[tokio::main]
async fn main() {
    if let Err(e) = run_analysis().await {
        eprintln!("Analysis failed: {}", e);
        std::process::exit(1);
    }
}

async fn run_analysis() -> Result<(), Box<dyn std::error::Error>> {
    // Extract repository path from args. If no args are given, default to the current directory.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let repo_path = if args.is_empty() { "." } else { &args[0] };

    // Use MockStorage if CARRICK_MOCK_ALL env var is set, otherwise use AWS Storage
    let use_mock = std::env::var("CARRICK_MOCK_ALL").is_ok();

    if use_mock {
        println!("Using MockStorage (CARRICK_MOCK_ALL environment variable detected)");
        let storage = MockStorage::new();
        run_analysis_engine(storage, repo_path).await
    } else {
        let storage = AwsStorage::new()?;
        run_analysis_engine(storage, repo_path).await
    }
}
