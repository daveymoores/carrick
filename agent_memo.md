

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

**Development Methodology**
- This is an MVP, so try to keep solutions minimal, providing enough functionality to meet MVP requirements but without the need for additonal market capture
- Do not write tests unless they are for temporary testing of technical solutions -> we are trying to generate a proof of concept
