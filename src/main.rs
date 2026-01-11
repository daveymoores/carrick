mod agent_service;
mod agents;
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
mod mount_graph;
mod multi_agent_orchestrator;
mod packages;
mod parser;
mod router_context;
mod services;
mod swc_scanner;
mod url_normalizer;
mod utils;
mod visitor;

use crate::cloud_storage::{AwsStorage, MockStorage};
use crate::services::TypeSidecar;
use engine::run_analysis_engine;
use std::env;
use std::path::Path;
use std::time::Duration;

/// CLI arguments for the carrick analyzer
struct CliArgs {
    /// Path to the repository to analyze
    repo_path: String,
    /// Enable sidecar-based type extraction
    enable_sidecar: bool,
    /// Path to the sidecar executable (defaults to CARRICK_SIDECAR_PATH env var)
    sidecar_path: Option<String>,
}

impl CliArgs {
    fn parse() -> Self {
        let args: Vec<String> = env::args().skip(1).collect();
        let mut repo_path = ".".to_string();
        let mut enable_sidecar = false;
        let mut sidecar_path: Option<String> = None;

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--sidecar-type-extraction" => {
                    enable_sidecar = true;
                }
                "--sidecar-path" => {
                    i += 1;
                    if i < args.len() {
                        sidecar_path = Some(args[i].clone());
                    }
                }
                "--help" | "-h" => {
                    Self::print_help();
                    std::process::exit(0);
                }
                arg if !arg.starts_with('-') => {
                    repo_path = arg.to_string();
                }
                _ => {
                    eprintln!("Unknown argument: {}", args[i]);
                    Self::print_help();
                    std::process::exit(1);
                }
            }
            i += 1;
        }

        // Check for sidecar path in environment variable
        if sidecar_path.is_none() {
            sidecar_path = env::var("CARRICK_SIDECAR_PATH").ok();
        }

        Self {
            repo_path,
            enable_sidecar,
            sidecar_path,
        }
    }

    fn print_help() {
        eprintln!(
            r#"Carrick - API Contract Analyzer

USAGE:
    carrick [OPTIONS] [REPO_PATH]

ARGUMENTS:
    [REPO_PATH]    Path to the repository to analyze (default: current directory)

OPTIONS:
    --sidecar-type-extraction    Enable sidecar-based TypeScript type extraction
    --sidecar-path <PATH>        Path to the sidecar executable
                                 (can also be set via CARRICK_SIDECAR_PATH env var)
    -h, --help                   Print this help message

ENVIRONMENT VARIABLES:
    CARRICK_API_KEY         API key for the LLM service (required)
    CARRICK_ORG             Organization name for cloud storage (required in CI)
    CARRICK_MOCK_ALL        Use mock storage instead of AWS
    CARRICK_SIDECAR_PATH    Path to the type-sidecar executable
    CARRICK_API_ENDPOINT    API endpoint for the carrick service (build-time)
"#
        );
    }
}

#[tokio::main]
async fn main() {
    if let Err(e) = run_analysis().await {
        eprintln!("Analysis failed: {}", e);
        std::process::exit(1);
    }
}

async fn run_analysis() -> Result<(), Box<dyn std::error::Error>> {
    let args = CliArgs::parse();

    // =======================================================================
    // STEP 1: Spawn sidecar FIRST (non-blocking) if enabled
    // =======================================================================
    let sidecar = if args.enable_sidecar {
        match spawn_sidecar(&args) {
            Ok(sidecar) => {
                eprintln!("[main] Sidecar spawned, initializing in background...");
                Some(sidecar)
            }
            Err(e) => {
                eprintln!("[main] Warning: Failed to spawn sidecar: {}", e);
                eprintln!("[main] Continuing without sidecar type extraction");
                None
            }
        }
    } else {
        None
    };

    // =======================================================================
    // STEP 2: Run analysis engine (SWC scanning + LLM analysis happens here)
    // These run in PARALLEL with sidecar initialization
    // =======================================================================

    // Use MockStorage if CARRICK_MOCK_ALL env var is set, otherwise use AWS Storage
    let use_mock = env::var("CARRICK_MOCK_ALL").is_ok();

    let result = if use_mock {
        println!("Using MockStorage (CARRICK_MOCK_ALL environment variable detected)");
        let storage = MockStorage::new();
        run_analysis_engine(storage, &args.repo_path).await
    } else {
        let storage = AwsStorage::new()?;
        run_analysis_engine(storage, &args.repo_path).await
    };

    // =======================================================================
    // STEP 3: Wait for sidecar if enabled (should already be ready by now)
    // =======================================================================
    if let Some(ref sidecar) = sidecar {
        eprintln!("[main] Waiting for sidecar to be ready...");
        match sidecar.wait_ready(Duration::from_secs(30)) {
            Ok(()) => {
                eprintln!("[main] Sidecar is ready for type extraction");
                // TODO: In Phase 2.4, we will integrate sidecar type resolution here
                // For now, just verify it's ready
            }
            Err(e) => {
                eprintln!("[main] Warning: Sidecar failed to initialize: {}", e);
                eprintln!("[main] Type extraction will be skipped");
            }
        }
    }

    // Sidecar will be automatically shut down when it goes out of scope (Drop impl)

    result
}

/// Spawn the type sidecar and start initialization
fn spawn_sidecar(args: &CliArgs) -> Result<TypeSidecar, Box<dyn std::error::Error>> {
    let sidecar_path = args
        .sidecar_path
        .as_ref()
        .ok_or("Sidecar path not specified. Use --sidecar-path or set CARRICK_SIDECAR_PATH")?;

    let path = Path::new(sidecar_path);

    // Spawn the sidecar process
    let sidecar = TypeSidecar::spawn(path)?;

    // Start initialization with the repo path
    let repo_path = Path::new(&args.repo_path);
    sidecar.start_init(repo_path, None);

    Ok(sidecar)
}
