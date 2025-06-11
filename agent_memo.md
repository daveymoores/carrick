

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


# Carrick - Cross-Repository API Analysis Tool - Status Summary

## Current State
The tool analyzes JavaScript/TypeScript APIs across multiple repositories and detects inconsistencies.

## What Just Got Fixed
1. ✅ **AWS Storage Implementation**: Completed AWS Lambda + DynamoDB + S3 integration
2. ✅ **CI Mode Pipeline**: Fixed type file generation and upload workflow
3. ✅ **Lambda Deployment**: Resolved action routing in Lambda functions
4. ✅ **Request/Response Mapping**: Fixed struct field naming mismatches

## Current Issue - ALMOST SOLVED
**DynamoDB Query Error**: `Query key condition not supported`

### The Problem
- Using `begins_with()` on partition key in DynamoDB Query operation (not allowed)
- Current table structure: `pk: repo#org/repo-name`, `sk: types#hash`
- Need to fetch all repos for an organization

### Immediate Fix Needed
Replace Query with Scan in `lambdas/check-or-upload/index.js`:

```javascript
// In handleGetCrossRepoData function, replace QueryCommand with:
const results = await docClient.send(
  new ScanCommand({
    TableName: TABLE_NAME,
    FilterExpression: "begins_with(pk, :orgPrefix) AND apiKey = :apiKey",
    ExpressionAttributeValues: {
      ":orgPrefix": `repo#${org}/`,
      ":apiKey": apiKey,
    },
  })
);

// Also add ScanCommand to imports:
const { ScanCommand } = require("@aws-sdk/lib-dynamodb");
```

### Deploy Steps
```bash
cd lambdas/
./build.sh
cd ../terraform/
terraform apply
```

## Architecture Overview
```
Rust App (CI Mode) → AWS API Gateway → Lambda → DynamoDB + S3
                                    ↓
                           Type files stored in S3
                           Metadata stored in DynamoDB
```

## Key Files
- `carrick/src/cloud_storage/aws_storage.rs` - AWS integration
- `carrick/lambdas/check-or-upload/index.js` - Lambda function (needs Scan fix)
- `carrick/src/ci_mode/mod.rs` - Cross-repo analysis orchestration

## Testing
- Set `CARRICK_ORG=carrick`, `CARRICK_API_KEY=xxx`, `CARRICK_LAMBDA_URL=xxx`
- Run: `cargo run -- --mode ci --path ./test-repo`

## Next Steps After Fix
1. Verify cross-repo data download works
2. Test type file downloading from S3
3. Validate end-to-end type checking across repos
4. Consider DynamoDB table restructure for better query performance

The tool is 95% complete - just needs the DynamoDB Scan fix to be fully functional.
