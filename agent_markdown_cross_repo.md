
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
- Single repo analysis with cloud data storage/retrieval
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
use crate::{ApiEndpointDetails, Mount, AppContext, FunctionDefinition};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CloudRepoData {
    pub repo_name: String,
    pub endpoints: Vec<ApiEndpointDetails>,
    pub calls: Vec<ApiEndpointDetails>,
    pub mounts: Vec<Mount>,
    pub apps: HashMap<String, AppContext>,
    pub imported_handlers: Vec<(String, String, String, String)>,
    pub function_definitions: HashMap<String, FunctionDefinition>,
    pub config_json: Option<String>, // Serialized carrick.json
    pub package_json: Option<String>, // Serialized package.json
    pub extracted_types: Vec<serde_json::Value>, // Pre-extracted type info
    pub last_updated: chrono::DateTime<chrono::Utc>,
    pub commit_hash: String,
}

pub trait CloudStorage {
    fn upload_repo_data(&self, token: &str, data: &CloudRepoData) -> Result<(), Box<dyn std::error::Error>>;
    fn download_all_repo_data(&self, token: &str) -> Result<Vec<CloudRepoData>, Box<dyn std::error::Error>>;
}
```

---

## **Step 3: Implement CI Mode Logic**

### **Add to `main.rs`:**
```rust
fn run_ci_mode() {
    let carrick_token = std::env::var("CARRICK_TOKEN")
        .expect("CARRICK_TOKEN must be set in CI mode");

    // 1. Analyze current repo only
    let current_repo_data = analyze_current_repo(".");

    // 2. Upload current repo data to cloud storage
    upload_repo_data(&carrick_token, &current_repo_data)
        .expect("Failed to upload repo data");

    // 3. Download data from all repos with same token
    let all_repo_data = download_all_repo_data(&carrick_token)
        .expect("Failed to download cross-repo data");

    // 4. Reconstruct analyzer with combined data
    let analyzer = build_cross_repo_analyzer(all_repo_data);

    // 5. Run analysis (same logic as local mode)
    let results = analyzer.get_results();

    // 6. Print results (same as local mode)
    print_results(results);
}
```

---

## **Step 4: Extract Current Repo Analysis**

### **Create `analyze_current_repo()` function:**
```rust
fn analyze_current_repo(repo_path: &str) -> CloudRepoData {
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

    // Find files in current repo only
    let ignore_patterns = ["node_modules", "dist", "build", ".next"];
    let (files, config_file_path, package_json_path) = find_files(repo_path, &ignore_patterns);

    // Process files and create visitors (same as current logic)
    let mut visitors = Vec::new();
    let mut processed_file_paths = HashSet::new();
    let mut file_queue = VecDeque::new();

    let repo_name = repo_path.split("/").last().unwrap_or("default");

    // Queue initial files
    for file_path in files {
        file_queue.push_back((file_path, repo_name.to_string(), None));
    }

    // Process queue (same as current logic)
    while let Some((file_path, repo_prefix, imported_router_name)) = file_queue.pop_front() {
        // ... existing file processing logic
    }

    // Create analyzer and extract data
    let config = Config::new(vec![config_file_path].into_iter().flatten().collect())
        .unwrap_or_default();
    let mut analyzer = Analyzer::new(config, cm.clone());

    for visitor in visitors {
        analyzer.add_visitor_data(visitor);
    }

    // Extract type information
    let extracted_types = extract_types_for_current_repo(&analyzer, repo_path);

    // Build CloudRepoData
    CloudRepoData {
        repo_name: repo_name.to_string(),
        endpoints: analyzer.endpoints,
        calls: analyzer.calls,
        mounts: analyzer.mounts,
        apps: analyzer.apps,
        imported_handlers: analyzer.imported_handlers,
        function_definitions: analyzer.function_definitions,
        config_json: serialize_config_if_exists(config_file_path),
        package_json: serialize_package_json_if_exists(package_json_path),
        extracted_types,
        last_updated: chrono::Utc::now(),
        commit_hash: get_current_commit_hash(),
    }
}
```

---

## **Step 5: Reconstruct Cross-Repo Analyzer**

### **Create `build_cross_repo_analyzer()` function:**
```rust
fn build_cross_repo_analyzer(all_repo_data: Vec<CloudRepoData>) -> Analyzer {
    // Combine all configs
    let combined_config = merge_configs(all_repo_data.iter().filter_map(|d| &d.config_json));

    // Create analyzer with combined config
    let cm: Lrc<SourceMap> = Default::default(); // Limited source map for CI mode
    let mut analyzer = Analyzer::new(combined_config, cm);

    // Populate analyzer with data from all repos
    for repo_data in all_repo_data {
        analyzer.endpoints.extend(repo_data.endpoints);
        analyzer.calls.extend(repo_data.calls);
        analyzer.mounts.extend(repo_data.mounts);
        analyzer.apps.extend(repo_data.apps);
        analyzer.imported_handlers.extend(repo_data.imported_handlers);
        analyzer.function_definitions.extend(repo_data.function_definitions);
    }

    // Resolve endpoint paths (same as local mode)
    let endpoints = analyzer.resolve_all_endpoint_paths(
        &analyzer.endpoints,
        &analyzer.mounts,
        &analyzer.apps
    );
    analyzer.endpoints = endpoints;

    // Build router
    analyzer.build_endpoint_router();

    analyzer
}
```

---

## **Step 6: Implement Cloud Storage Backend**

### **Choose a storage backend (AWS S3, Google Cloud Storage, etc.):**
```rust
// Example with AWS S3
pub struct S3CloudStorage {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl CloudStorage for S3CloudStorage {
    fn upload_repo_data(&self, token: &str, data: &CloudRepoData) -> Result<(), Box<dyn std::error::Error>> {
        let key = format!("{}/{}.json", token, data.repo_name);
        let json_data = serde_json::to_string(data)?;

        // Upload to S3
        // Implementation depends on chosen cloud provider
        Ok(())
    }

    fn download_all_repo_data(&self, token: &str) -> Result<Vec<CloudRepoData>, Box<dyn std::error::Error>> {
        // List all objects with token prefix
        // Download and deserialize each repo's data
        // Return combined data
        Ok(vec![])
    }
}
```

---

## **Step 7: GitHub Actions Integration**

### **Create `.github/workflows/carrick.yml`:**
```yaml
name: Carrick API Analysis
on:
  push:
    branches: [main]

jobs:
  analyze:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Install Node.js (for TypeScript checking)
        uses: actions/setup-node@v3
        with:
          node-version: '18'

      - name: Run Carrick Analysis
        env:
          CARRICK_TOKEN: ${{ secrets.CARRICK_TOKEN }}
          # Add cloud storage credentials
          AWS_ACCESS_KEY_ID: ${{ secrets.AWS_ACCESS_KEY_ID }}
          AWS_SECRET_ACCESS_KEY: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
        run: |
          # Build and run carrick
          cargo run
```

---

## **Step 8: Token Management**

### **Token Generation Strategy:**
- Create a shared secret/token for each group of related repositories
- Store as GitHub repository secret: `CARRICK_TOKEN`
- Same token across all repos that should analyze together
- Consider format: `project-name-uuid` (e.g., `ecommerce-a1b2c3d4`)

---

## **Step 9: Type Extraction for CI Mode**

### **Modify type extraction to work with cloud data:**
```rust
fn extract_types_for_current_repo(analyzer: &Analyzer, repo_path: &str) -> Vec<serde_json::Value> {
    // Use existing extract_types_for_repo but capture the type data
    // instead of just writing files

    // This should return structured type information that can be
    // serialized and later used for type checking in other CI runs
    vec![]
}
```

---

## **Step 10: Testing Strategy**

### **Local Testing:**
1. Set up two test repositories
2. Run in local mode to verify cross-repo analysis works
3. Set `CI=true` and `CARRICK_TOKEN=test-token`
4. Run each repo individually and verify cloud storage integration

### **CI Testing:**
1. Set up GitHub repositories with the workflow
2. Configure shared `CARRICK_TOKEN` secret
3. Push to main in different repos and verify cross-repo analysis

---

## **Step 11: Configuration**

### **Add CI-specific configuration to `carrick.json`:**
```json
{
  "cloud_storage": {
    "provider": "s3",
    "bucket": "carrick-analysis-data",
    "region": "us-east-1"
  },
  "ci_mode": {
    "timeout_seconds": 300,
    "max_repo_wait": 10
  }
}
```

---

## **Key Implementation Files to Create/Modify:**

1. **`src/main.rs`** - Add CI/local mode detection and routing
2. **`src/cloud_storage.rs`** - Cloud storage abstraction and implementation
3. **`src/ci_mode.rs`** - CI-specific logic (analyze_current_repo, build_cross_repo_analyzer)
4. **`Cargo.toml`** - Add cloud storage and serialization dependencies
5. **`.github/workflows/carrick.yml`** - GitHub Actions workflow

---

## **Expected Behavior:**

**Initial repo run:**
```
⚠️  Found 19 API issues:
- Many orphaned endpoints/calls (expected - other repos haven't run yet)
- Type mismatches (expected - consumer/producer not yet paired)
```

**After all repos run:**
```
✅ All types are compatible!
⚠️  Found 2 API issues:
- Orphaned endpoints: /ping, /metrics (intentionally orphaned for monitoring)
```

This progressive issue resolution is a **feature**, not a bug - it shows real cross-repo integration progress.
