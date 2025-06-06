

# Carrick - Cross-Repository API Analysis Tool

## Project Overview

Carrick is a Rust-based tool that analyzes JavaScript/TypeScript codebases to detect API inconsistencies across multiple repositories. It identifies mismatches between API endpoints and their callers, performs type checking, and helps maintain API compatibility in microservices architectures.

## Core Functionality

**Local Mode**: Analyzes a single repository for internal API consistency
**CI Mode**: Performs cross-repository analysis by sharing type information and API definitions between repos

The tool:
- Parses JavaScript/TypeScript files using SWC
- Extracts API endpoint definitions (Express routes, handlers)
- Identifies API calls (fetch, axios, etc.)
- Performs type checking and validates request/response schemas
- Detects mismatches between API providers and consumers across repos

## Architecture

### Core Components
- **Parser**: Uses SWC to parse JS/TS files into ASTs
- **Visitor**: Walks ASTs to extract API endpoints, calls, and type information
- **Analyzer**: Matches endpoints with calls, resolves types, identifies issues
- **Cloud Storage**: Abstraction for sharing data between repositories

### Data Flow
1. Parse files in current repository
2. Extract endpoints, calls, types, and metadata
3. Upload current repo data to cloud storage
4. Download data from other repositories with same API key
5. Perform cross-repo analysis and report inconsistencies

## Current Implementation Status

### Cloud Storage Migration (Recently Completed)
We've **migrated from MongoDB to AWS** for better scalability:

**Previous (MongoDB)**:
- Direct MongoDB connections
- All data stored in single database
- Type files embedded in documents

**Current (AWS)**:
- **API Gateway** + **Lambda** for REST API interface
- **DynamoDB** for metadata storage (repo info, endpoints, calls)
- **S3** for type files (larger TypeScript definitions)
- **Single Lambda** (`check-or-upload`) handles all operations via action parameter

### Key Files Structure

```
carrick/
├── src/
│   ├── analyzer/           # Core analysis logic
│   ├── visitor/           # AST walking and data extraction
│   ├── cloud_storage/     # Storage abstraction layer
│   │   ├── mod.rs        # CloudStorage trait definition
│   │   ├── aws_storage.rs # NEW: AWS implementation
│   │   ├── mongodb_storage.rs # Legacy MongoDB (kept for compatibility)
│   │   └── mock_storage.rs    # Testing implementation
│   ├── ci_mode/          # Cross-repo analysis orchestration
│   └── parser/           # SWC-based file parsing
├── lambdas/
│   └── check-or-upload/   # AWS Lambda handling all storage operations
└── terraform/            # AWS infrastructure as code
```

## AWS Infrastructure

### Lambda Function (`check-or-upload`)
**Actions handled**:
- `check-or-upload`: Check if type file exists, return upload URL if needed + adjacent repos
- `store-metadata`: Store CloudRepoData metadata in DynamoDB
- `complete-upload`: Verify S3 upload + store metadata (with validation)
- `get-cross-repo-data`: Retrieve all repos for an organization

### Data Models

**DynamoDB Schema**:
```
PK: repo#${org}/${repo}
SK: types#${commit_hash}
```

**CloudRepoData Structure**:
```rust
pub struct CloudRepoData {
    pub repo_name: String,
    pub endpoints: Vec<ApiEndpointDetails>,    // API definitions
    pub calls: Vec<ApiEndpointDetails>,        // API calls found
    pub mounts: Vec<Mount>,                    // Router mounts
    pub apps: HashMap<String, AppContext>,    // Express apps
    pub imported_handlers: Vec<(String, String, String, String)>,
    pub function_definitions: HashMap<String, FunctionDefinition>,
    pub config_json: Option<String>,
    pub package_json: Option<String>,
    pub extracted_types: Vec<serde_json::Value>, // Type definitions
    pub last_updated: DateTime<Utc>,
    pub commit_hash: String,
}
```

## Current State & Next Steps

### ✅ Recently Completed
- AWS infrastructure setup (Terraform)
- Lambda function with multi-action support
- AWS storage implementation (`AwsStorage`)
- DynamoDB + S3 integration
- API Gateway configuration

### 🔧 Current Status
The migration is **functionally complete** but needs:

1. **CI Mode Integration**: Update `ci_mode/mod.rs` to use `AwsStorage` instead of MongoDB
2. **Environment Setup**: Switch from `CARRICK_TOKEN`/`MONGODB_URI` to `CARRICK_API_KEY`/`CARRICK_LAMBDA_URL`
3. **Testing**: Validate end-to-end cross-repo analysis workflow

### 🎯 Immediate Tasks
- [ ] Add `run_ci_mode_aws()` function or update main.rs to use AWS storage
- [ ] Test lambda deployment and API Gateway integration
- [ ] Verify cross-repo type sharing workflow
- [ ] Update documentation for new AWS setup

### 🔮 Future Enhancements
- Enhanced type checking across repos
- Better error reporting and debugging
- UI for visualizing API dependencies
- Integration with CI/CD pipelines
- Support for additional frameworks beyond Express

## Development Setup

**Local Development**:
```bash
# Environment variables
export CARRICK_LAMBDA_URL="https://your-api-gateway.com/types/check-or-upload"
export CARRICK_API_KEY="your-secure-api-key"

# Run analysis
cargo run -- --mode ci --path ./your-repo
```

**AWS Deployment**:
```bash
cd terraform/
terraform apply

cd ../lambdas/check-or-upload/
npm run build
# Upload via terraform or AWS CLI
```

The project is at a **critical transition point** - the AWS infrastructure is ready, but the Rust application needs final integration to complete the migration from MongoDB to the new scalable AWS architecture.


---

Looking at your code, it looks correct overall! Here's what you should do:

## Yes, you can remove MongoDB storage (if you want)

Since you're moving to AWS, you can remove `mongodb_storage.rs` if you don't need backward compatibility. But I'd recommend keeping it for now in case you need to roll back.

## Update your module exports

Your `aws_storage.rs` looks good, but you need to update `carrick/src/cloud_storage/mod.rs`:

```rust
// carrick/src/cloud_storage/mod.rs
use crate::{
    analyzer::ApiEndpointDetails,
    app_context::AppContext,
    visitor::{FunctionDefinition, Mount},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;

mod mock_storage;
mod mongodb_storage;
mod aws_storage; // Add this line

pub use mock_storage::MockStorage;
pub use mongodb_storage::MongoStorage;
pub use aws_storage::AwsStorage; // Add this line

// ... rest of the file remains the same
```

## Update CI mode to use AWS storage

Your `ci_mode/mod.rs` is still hardcoded for MongoDB. You need to either:

### Option A: Add a new AWS-specific function

```rust
// Add this to carrick/src/ci_mode/mod.rs

use crate::cloud_storage::AwsStorage; // Add this import

pub async fn run_ci_mode_aws(
    repo_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Running Carrick in CI mode with AWS backend");

    // Initialize AWS storage
    let storage = AwsStorage::new()?;

    // Verify connectivity early
    storage
        .health_check()
        .await
        .map_err(|e| format!("Failed to connect to AWS services: {}", e))?;
    println!("AWS connectivity verified");

    // Use empty token since AwsStorage uses API key from env
    let token = "";

    // 1. Analyze current repo only
    let current_repo_data = analyze_current_repo(repo_path)?;
    println!("Analyzed current repo: {}", current_repo_data.repo_name);

    // 2. Upload current repo data to AWS
    let cloud_data_serialized = serialize_cloud_repo_data_without_ast(&current_repo_data);
    storage
        .upload_repo_data(token, &cloud_data_serialized)
        .await
        .map_err(|e| format!("Failed to upload repo data: {}", e))?;
    println!("Uploaded current repo data to AWS");

    // 3. Download data from all repos
    let all_repo_data = storage
        .download_all_repo_data(token)
        .await
        .map_err(|e| format!("Failed to download cross-repo data: {}", e))?;
    println!("Downloaded data from {} repos", all_repo_data.len());

    // 4. Reconstruct analyzer with combined data
    let analyzer = build_cross_repo_analyzer(all_repo_data)?;
    println!("Reconstructed analyzer with cross-repo data");

    // 5. Run analysis
    let results = analyzer.get_results();

    // 6. Print results
    print_results(results);

    Ok(())
}
```

### Option B: Update your main.rs to use AWS storage

```rust
// In your main.rs or wherever you call CI mode
match run_mode.as_str() {
    "ci" => {
        // Use new AWS-based CI mode
        crate::ci_mode::run_ci_mode_aws(&args.path).await?;
    }
    "ci-mongo" => {
        // Keep MongoDB option for backward compatibility
        let storage = MongoStorage::new().await?;
        crate::ci_mode::run_ci_mode(storage, &args.path).await?;
    }
    // ... other modes
}
```

## Fix one small issue in upload_type_file

Your `upload_type_file` method in `aws_storage.rs` is missing the `s3Url` field:

```rust
async fn upload_type_file(
    &self,
    _token: &str,
    repo_name: &str,
    file_name: &str,
    content: &str,
) -> Result<(), StorageError> {
    let (org, repo) = self.extract_org_and_repo(repo_name);
    let commit_hash = crate::cloud_storage::get_current_commit_hash();

    let request = LambdaRequest {
        action: "check-or-upload".to_string(),
        repo,
        org,
        hash: commit_hash,
        filename: file_name.to_string(),
        cloudRepoData: None,
        s3Url: None, // Add this line
    };

    let lambda_response: LambdaResponse = self.call_lambda(&request).await?;

    if let Some(upload_url) = lambda_response.upload_url {
        self.upload_to_s3(&upload_url, content).await?;
    }

    Ok(())
}
```

## Environment Variables

Make sure your environment is set up correctly:

```bash
# For AWS (new)
export CARRICK_LAMBDA_URL="https://your-api-gateway-url/types/check-or-upload"
export CARRICK_API_KEY="your-api-key"

# Remove MongoDB variables (if not needed for backward compatibility)
# unset MONGODB_URI
# unset CARRICK_TOKEN
```

## Summary

Your AWS implementation looks great! The main things to do are:

1. ✅ **Your `aws_storage.rs` is correct**
2. ✅ **Your lambda looks good**
3. 🔧 **Add `AwsStorage` export to `mod.rs`**
4. 🔧 **Add the `s3Url: None` line to `upload_type_file`**
5. 🔧 **Add `run_ci_mode_aws()` function or update your main.rs**
6. 🔧 **Set the correct environment variables**

You can keep the MongoDB storage for now (good for backward compatibility), or remove it if you're confident in the AWS migration.

The architecture looks solid - you've successfully moved from MongoDB to a much more scalable AWS solution!
