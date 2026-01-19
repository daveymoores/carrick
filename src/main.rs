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
mod type_manifest;
mod url_normalizer;
mod utils;
mod visitor;
mod wrapper_registry;

use crate::cloud_storage::{AwsStorage, MockStorage};
use crate::services::TypeSidecar;
use engine::run_analysis_engine_with_sidecar;
use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// CLI arguments for the carrick analyzer
struct CliArgs {
    /// Path to the repository to analyze
    repo_path: String,
}

impl CliArgs {
    fn parse() -> Self {
        let args: Vec<String> = env::args().skip(1).collect();
        let mut repo_path = ".".to_string();

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
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

        Self { repo_path }
    }

    fn print_help() {
        eprintln!(
            r#"Carrick - API Contract Analyzer

USAGE:
    carrick [OPTIONS] [REPO_PATH]

ARGUMENTS:
    [REPO_PATH]    Path to the repository to analyze (default: current directory)

OPTIONS:
    -h, --help     Print this help message

ENVIRONMENT VARIABLES:
    CARRICK_API_KEY         API key for the LLM service (required)
    CARRICK_ORG             Organization name for cloud storage (required in CI)
    CARRICK_MOCK_ALL        Use mock storage instead of AWS
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
    // STEP 1: Discover and spawn sidecar (non-blocking)
    // The sidecar is bundled with the tool - auto-discover its location
    // =======================================================================
    let sidecar = match discover_sidecar_path() {
        Some(sidecar_path) => {
            eprintln!("[main] Found sidecar at: {}", sidecar_path.display());
            match spawn_sidecar(&sidecar_path, &args.repo_path) {
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
        }
        None => {
            eprintln!("[main] Sidecar not found, continuing without type extraction");
            eprintln!("[main] (Run 'npm run build' in src/sidecar to enable type extraction)");
            None
        }
    };

    // =======================================================================
    // STEP 2: Wait for sidecar to be ready (if spawned) before analysis
    // The sidecar initializes in parallel, so it should be ready by now
    // =======================================================================
    let sidecar_ready = if let Some(ref sidecar) = sidecar {
        eprintln!("[main] Waiting for sidecar to be ready...");
        match sidecar.wait_ready(Duration::from_secs(30)) {
            Ok(()) => {
                eprintln!("[main] Sidecar is ready for type extraction");
                true
            }
            Err(e) => {
                eprintln!("[main] Warning: Sidecar failed to initialize: {}", e);
                eprintln!("[main] Continuing without sidecar type extraction");
                false
            }
        }
    } else {
        false
    };

    // =======================================================================
    // STEP 3: Run analysis engine with sidecar (if ready)
    // =======================================================================

    // Use MockStorage if CARRICK_MOCK_ALL env var is set, otherwise use AWS Storage
    let use_mock = env::var("CARRICK_MOCK_ALL").is_ok();

    // Pass sidecar reference if it's ready
    let sidecar_ref = if sidecar_ready {
        sidecar.as_ref()
    } else {
        None
    };

    if use_mock {
        println!("Using MockStorage (CARRICK_MOCK_ALL environment variable detected)");
        let storage = MockStorage::new();
        run_analysis_engine_with_sidecar(storage, &args.repo_path, sidecar_ref).await
    } else {
        let storage = AwsStorage::new()?;
        run_analysis_engine_with_sidecar(storage, &args.repo_path, sidecar_ref).await
    }

    // Sidecar will be automatically shut down when it goes out of scope (Drop impl)
}

/// Discover the sidecar path by checking known locations
fn discover_sidecar_path() -> Option<PathBuf> {
    // The sidecar entry point after building (TypeScript compiles to dist/src/)
    let sidecar_entry = "dist/src/index.js";

    // List of locations to check, in order of priority
    let mut candidates: Vec<PathBuf> = vec![
        // 1. Relative to executable (for packaged distribution)
        get_executable_relative_path("sidecar"),
        get_executable_relative_path("../sidecar"),
        get_executable_relative_path("../lib/sidecar"),
    ];

    // 2. For development builds, use CARGO_MANIFEST_DIR (set at compile time)
    //    This ensures we find the sidecar regardless of the current working directory
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(PathBuf::from(manifest_dir).join("src/sidecar"));
    }

    // 3. Fallback to relative paths (in case running from project root)
    candidates.extend([
        PathBuf::from("src/sidecar"),
        PathBuf::from("./src/sidecar"),
        PathBuf::from("sidecar"),
    ]);

    for candidate in candidates {
        let full_path = candidate.join(sidecar_entry);
        if full_path.exists() {
            eprintln!("[main] Checking sidecar candidate: {:?}", full_path);
            return Some(full_path);
        }
    }

    None
}

/// Get a path relative to the executable location
fn get_executable_relative_path(relative: &str) -> PathBuf {
    if let Ok(exe_path) = env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            return exe_dir.join(relative);
        }
    }
    PathBuf::from(relative)
}

/// Spawn the type sidecar and start initialization
fn spawn_sidecar(
    sidecar_path: &Path,
    repo_path: &str,
) -> Result<TypeSidecar, Box<dyn std::error::Error>> {
    // Convert repo path to absolute path for the sidecar
    // The sidecar runs as a separate process and needs an absolute path
    let repo_path = Path::new(repo_path);
    let absolute_repo_path = if repo_path.is_absolute() {
        repo_path.to_path_buf()
    } else {
        // Get current working directory and join with relative path
        let cwd = env::current_dir()
            .map_err(|e| format!("Failed to get current working directory: {}", e))?;
        eprintln!("[main] Current working directory: {:?}", cwd);
        eprintln!("[main] Repo path (relative): {:?}", repo_path);
        cwd.join(repo_path)
    };

    eprintln!(
        "[main] Repo path (before canonicalize): {:?}",
        absolute_repo_path
    );

    // Canonicalize to resolve any .. or . segments in the path
    let absolute_repo_path = absolute_repo_path.canonicalize().map_err(|e| {
        format!(
            "Failed to canonicalize repo path '{}': {}. \
            Make sure the path exists and you're running from the correct directory.",
            absolute_repo_path.display(),
            e
        )
    })?;

    eprintln!("[main] Repo path (canonicalized): {:?}", absolute_repo_path);

    // Spawn the sidecar process
    let sidecar = TypeSidecar::spawn(sidecar_path)?;

    sidecar.start_init(&absolute_repo_path, None);

    Ok(sidecar)
}
