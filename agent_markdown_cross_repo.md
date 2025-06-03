# Carrick CI Implementation: Complete Step-by-Step Guide

## **Context Overview**
Carrick is a Rust program that analyzes API consistency across multiple JavaScript/TypeScript repositories. Currently, it takes multiple repo paths as arguments and performs local cross-repo analysis. The goal is to extend it to work in CI mode where each repo runs independently but can still perform cross-repo analysis via cloud storage.

## **Current Architecture (Local Mode)**
- Takes multiple repo directories as CLI args
- Creates `DependencyVisitor` for each file across all repos
- Collects all visitors into an `Analyzer`
- Runs `analyze_api_consistency()` with all data
- Performs type checking using extracted TypeScript files

## **Target Architecture (CI Mode)**
- Detect CI environment (`CI=true` in GitHub Actions)
- Single repo analysis with cloud data exchange
- Token-based cross-repo data linking
- Same analysis logic but with reconstructed `Analyzer` from cloud data

---

## **Step 1: Add CI Mode Detection**

### **Modify `main.rs`:**
```rust
fn main() {
    let is_ci_mode = std::env::var("CI").is_ok();

    if is_ci_mode {
        run_ci_mode();
    } else {
        run_local_mode(); // Current implementation
    }
}

fn run_local_mode() {
    // Move your current main() logic here unchanged
    // This preserves existing local multi-repo functionality
}

fn run_ci_mode() {
    // New CI implementation (Step 2)
}
```

---

## **Step 2: Design Cloud Storage Data Structure**

### **Create `cloud_storage.rs` module:**
```rust
use serde::{Deserialize, Serialize};
use crate::{ApiEndpointDetails, Mount, AppContext, FunctionDefinition, visitor::OwnerType};
use std::collections::HashMap;
use std::path::PathBuf;
use chrono::{DateTime, Utc};

/// Data structure for storing repository analysis results in cloud storage
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudRepoData {
    pub repo_name: String,
    
    // API Metadata (without AST nodes)
    pub endpoints: Vec<ApiEndpointDetailsSerialized>,
    pub calls: Vec<ApiEndpointDetailsSerialized>,
    pub mounts: Vec<Mount>,
    pub apps: HashMap<String, AppContext>,
    pub imported_handlers: Vec<(String, String, String, String)>,
    
    // Configuration
    pub config_json: Option<String>, // Serialized carrick.json
    pub package_json: Option<String>, // Serialized package.json
    
    // Pre-extracted type info
    pub extracted_types: Vec<serde_json::Value>,
    
    // Metadata
    pub last_updated: DateTime<Utc>,
    pub commit_hash: String,
}

/// Serializable version of ApiEndpointDetails without AST nodes
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ApiEndpointDetailsSerialized {
    pub owner: Option<OwnerType>,
    pub route: String,
    pub method: String,
    pub params: Vec<String>,
    pub request_body: Option<Json>,
    pub response_body: Option<Json>,
    pub handler_name: Option<String>,
    pub file_path: PathBuf,
    // No request_type or response_type - these contain AST nodes that don't serialize well
}

/// Function to convert between ApiEndpointDetails and serializable version
impl From<ApiEndpointDetails> for ApiEndpointDetailsSerialized {
    fn from(api: ApiEndpointDetails) -> Self {
        Self {
            owner: api.owner,
            route: api.route,
            method: api.method,
            params: api.params,
            request_body: api.request_body,
            response_body: api.response_body,
            handler_name: api.handler_name,
            file_path: api.file_path,
            // Intentionally omit request_type and response_type
        }
    }
}

/// Cloud storage trait defining the interface for storing and retrieving data
pub trait CloudStorage {
    fn upload_repo_data(&self, token: &str, data: &CloudRepoData) -> Result<(), Box<dyn std::error::Error>>;
    fn download_all_repo_data(&self, token: &str) -> Result<Vec<CloudRepoData>, Box<dyn std::error::Error>>;
    fn upload_type_file(&self, token: &str, repo_name: &str, file_name: &str, content: &str) -> Result<(), Box<dyn std::error::Error>>;
    fn download_all_type_files(&self, token: &str, output_dir: &str) -> Result<Vec<String>, Box<dyn std::error::Error>>;
    fn health_check(&self) -> Result<(), Box<dyn std::error::Error>>;
}

/// Helper function to get current git commit hash
pub fn get_current_commit_hash() -> String {
    std::process::Command::new("git")
        .args(&["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok().map(|s| s.trim().to_string())
            } else {
                