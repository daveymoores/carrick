# Cloud Infrastructure Research: Multi-Repo Dependency Analysis

This document provides a comprehensive analysis of how Carrick uses cloud services for multi-repo dependency analysis.

## AWS Services Architecture

### 1. S3 (Simple Storage Service)
- **Purpose**: Stores TypeScript type definition files for each repository
- **Key Structure**: `{org}/{repo}/{commit_hash}/{filename}`
- **Security**: Public access completely blocked via `aws_s3_bucket_public_access_block`
- **Access**: Pre-signed URLs with 5-minute expiration for secure uploads

### 2. DynamoDB
- **Billing**: Pay-per-request (no provisioned capacity)
- **Schema**:
  - **Partition Key (pk)**: `repo#{org}/{repo}`
  - **Sort Key (sk)**: `types`
  - **TTL**: 30 days (items auto-expire)
- **Purpose**: Stores metadata about each repository including:
  - S3 URL for type files
  - Commit hash
  - CloudRepoData (endpoints, API calls, mounts, packages, dependencies)
  - Repository organization and name
  - Timestamps (createdAt, updatedAt)

### 3. Lambda Functions
Two Node.js Lambda functions handle all cloud operations:

#### Lambda 1: check-or-upload
- **Runtime**: Node.js 22.x
- **Timeout**: 30 seconds
- **Actions Supported**:
  - `check-or-upload`: Check if types exist or generate upload URL
  - `store-metadata`: Store repository metadata in DynamoDB
  - `complete-upload`: Validate S3 upload and save metadata
  - `get-cross-repo-data`: Retrieve all repositories in an organization
  - `download-file`: Download type files from S3 via proxy

- **Permissions**:
  - DynamoDB: GetItem, PutItem, Query, Scan
  - S3: PutObject, GetObject, HeadObject, ListBucket
  - CloudWatch: Log writing

#### Lambda 2: gemini-proxy
- **Runtime**: Node.js 22.x
- **Timeout**: 60 seconds
- **Purpose**: Proxy requests to Google Gemini API for AI-powered code analysis
- **Rate Limiting**: 2000 requests/day (resets at midnight UTC)
- **Features**:
  - Message conversion (system/user/assistant roles)
  - Request validation (max 5MB, max 10 messages)
  - CORS support for web clients

### 4. API Gateway (HTTP API - v2)
- **Type**: HTTP API (not REST API - simpler and cheaper)
- **Auto-deploy**: Enabled on `$default` stage
- **Endpoints**:
  - `POST /types/check-or-upload` → check-or-upload Lambda
  - `POST /gemini/chat` → gemini-proxy Lambda
  - `OPTIONS /gemini/chat` → CORS preflight
- **Security**: TLS 1.2, API key authentication (Bearer tokens)

### 5. CloudWatch
- **Log Groups**: Automatic logging for both Lambda functions
- **Alarms**: SNS notification if check-or-upload Lambda invocations exceed 20/minute
- **Monitoring**: Response times, error rates, token usage (for Gemini)

### 6. ACM (AWS Certificate Manager)
- **Purpose**: SSL/TLS certificate for custom domain
- **Validation**: DNS validation method
- **Security Policy**: TLS 1.2 minimum

### 7. SNS (Simple Notification Service)
- **Purpose**: Alert on excessive Lambda usage via email notifications

---

## Architecture and Data Flow

### Upload Flow (Main Branch Only)

```
1. Carrick CLI (Rust) runs on CI/CD
   ↓
2. Analyzes current repository
   - Parses TypeScript/JavaScript files
   - Extracts API endpoints, calls, dependencies
   - Generates type definitions
   - Collects package.json data
   ↓
3. Gets current git commit hash
   ↓
4. Calls Lambda: check-or-upload
   Request: { action: "check-or-upload", org, repo, hash, filename }
   ↓
5. Lambda checks DynamoDB for existing record
   - If hash matches: returns existing S3 URL
   - If hash differs or missing: generates pre-signed S3 URL
   ↓
6. If upload needed:
   a. Rust code uploads types.ts to S3 via pre-signed URL
   b. Calls Lambda: complete-upload
      Request: { action: "complete-upload", org, repo, hash, s3Url, filename, cloudRepoData }
   c. Lambda validates S3 file exists (HeadObject)
   d. Lambda stores full metadata in DynamoDB
   ↓
7. If only metadata update needed:
   Calls Lambda: store-metadata
   Request: { action: "store-metadata", org, repo, hash, filename, cloudRepoData }
```

### Download Flow (Every Run - PRs and Main)

```
1. Carrick CLI calls Lambda: get-cross-repo-data
   Request: { action: "get-cross-repo-data", org }
   ↓
2. Lambda scans DynamoDB with pagination
   Filter: all repos starting with "repo#{org}/"
   ↓
3. Lambda returns array of repositories with:
   - Repository name
   - Commit hash
   - S3 URL for type file
   - Full CloudRepoData (endpoints, calls, packages, etc.)
   ↓
4. Rust code processes all repos:
   - Filters out current repo (avoid duplicates)
   - Rebuilds Analyzer with combined data
   - Compares API endpoints across repos
   - Detects dependency version conflicts
   ↓
5. For type file downloads (if needed):
   Calls Lambda: download-file
   Request: { action: "download-file", s3Url }
   Lambda proxies S3 GetObject and returns content
```

### Branch Detection (Smart Upload Logic)

The Rust code checks environment variables to determine upload behavior:
- **Upload enabled**: main/master branches, local development
- **Upload disabled**: Pull requests, feature branches
- **Detection via**: `GITHUB_EVENT_NAME`, `GITHUB_REF` environment variables
- **Rationale**: PRs should analyze only, not pollute production cache

---

## Rust Cloud Storage Implementation

### Trait-Based Design

The Rust codebase uses a trait-based architecture for flexibility:

**`CloudStorage` Trait** (`src/cloud_storage/mod.rs`)
```rust
pub trait CloudStorage {
    async fn upload_repo_data(&self, org: &str, data: &CloudRepoData) -> Result<(), StorageError>;
    async fn download_all_repo_data(&self, org: &str) -> Result<(Vec<CloudRepoData>, HashMap<String, String>), StorageError>;
    async fn upload_type_file(&self, repo_name: &str, file_name: &str, content: &str) -> Result<(), StorageError>;
    async fn health_check(&self) -> Result<(), StorageError>;
    async fn download_type_file_content(&self, s3_url: &str) -> Result<String, StorageError>;
}
```

### Two Implementations

#### 1. AwsStorage (`src/cloud_storage/aws_storage.rs`)
- **HTTP Client**: `reqwest` for Lambda API calls
- **Authentication**: Bearer token from `CARRICK_API_KEY` environment variable
- **Endpoint**: Compile-time constant for API endpoint
- **Request Types**:
  - `LambdaRequest`: Serializes to JSON with snake_case → camelCase conversion
  - Generic request/response handling with proper error mapping
- **Upload Logic**:
  1. Check if upload needed (Lambda returns pre-signed URL or existing URL)
  2. If needed: Upload to S3 via PUT with pre-signed URL
  3. Complete upload: Store metadata in DynamoDB via Lambda
- **Download Logic**:
  1. Calls `get-cross-repo-data` action
  2. Parses `CrossRepoResponse` with array of `AdjacentRepo`
  3. Returns tuple: (Vec of CloudRepoData, HashMap of S3 URLs)

#### 2. MockStorage (`src/cloud_storage/mock_storage.rs`)
- **Purpose**: Local testing without AWS
- **Storage**: In-memory `Mutex<HashMap>` for data
- **Mock Data**: Pre-configured repos with dependency conflicts
  - repo-a: express 5.0.0, react 18.3.0, lodash 4.17.22
  - repo-b: express 4.18.0, react 18.2.0, lodash 4.17.21
- **Conflict Testing**: Simulates major, minor, and patch version differences
- **Activation**: Set `CARRICK_MOCK_ALL` environment variable

### CloudRepoData Structure

The core data structure stored and retrieved:

```rust
pub struct CloudRepoData {
    pub repo_name: String,
    pub endpoints: Vec<ApiEndpointDetails>,        // API endpoints defined in repo
    pub calls: Vec<ApiEndpointDetails>,            // API calls made by repo
    pub mounts: Vec<Mount>,                        // Express/framework mounts
    pub apps: HashMap<String, AppContext>,         // App contexts
    pub imported_handlers: Vec<(String, String, String, String)>,
    pub function_definitions: HashMap<String, FunctionDefinition>,
    pub config_json: Option<String>,               // Carrick config
    pub package_json: Option<String>,              // Raw package.json
    pub packages: Option<Packages>,                // Parsed dependencies
    pub last_updated: DateTime<Utc>,
    pub commit_hash: String,
}
```

**Key Point**: AST nodes are stripped before upload (request_type, response_type removed) to reduce payload size and avoid serialization issues.

---

## How Dependencies Are Stored and Retrieved

### Storage Format

Dependencies are stored in multiple layers:

1. **Raw package.json**: String stored in `CloudRepoData.package_json`
2. **Parsed Packages**: Structured `Packages` object in `CloudRepoData.packages`
3. **Merged Dependencies**: `HashMap<String, PackageInfo>` with resolved versions

**PackageInfo Structure**:
```rust
pub struct PackageInfo {
    pub name: String,           // e.g., "express"
    pub version: String,        // e.g., "5.0.0" (cleaned, no ^/~)
    pub source_path: PathBuf,   // Origin file for tracking
}
```

### Retrieval and Analysis

When downloading cross-repo data:

1. **Lambda returns full metadata** including `packages` field
2. **Rust deserializes** into `Vec<CloudRepoData>`
3. **Analyzer processes** all repositories:
   ```rust
   pub fn analyze_dependencies(&self) -> Vec<DependencyConflict>
   ```
4. **Conflict Detection**:
   - Groups packages by name across all repos
   - Compares versions using semver parsing
   - Categorizes conflicts:
     - **Critical**: Major version differences (5.x vs 4.x)
     - **Warning**: Minor version differences (18.3.x vs 18.2.x)
     - **Info**: Patch version differences (4.17.22 vs 4.17.21)

**DependencyConflict Structure**:
```rust
pub struct DependencyConflict {
    pub package_name: String,
    pub versions: Vec<(String, String)>,     // (repo_name, version)
    pub severity: ConflictSeverity,          // Critical/Warning/Info
    pub description: String,
}
```

---

## Caching and Optimization Strategies

### 1. Hash-Based Caching
- **Strategy**: Only upload when commit hash changes
- **Benefit**: Avoids redundant uploads for unchanged code
- **Implementation**: Lambda checks DynamoDB for existing hash before generating upload URL
- **Invalidation**: Automatic on new commits

### 2. DynamoDB Query Optimization
- **Partition Key Design**: `repo#{org}/{repo}` enables efficient org-level queries
- **Sort Key**: `types` allows future expansion (could add `types#{hash}` for versioning)
- **Scan with Filter**: Uses `begins_with(pk, "repo#{org}/")` for cross-repo queries
- **Pagination**: Lambda handles pagination automatically for large organizations

### 3. S3 Pre-Signed URLs
- **Benefit**: No Lambda proxy for large file uploads
- **Security**: 5-minute expiration limits exposure
- **Performance**: Direct client-to-S3 upload, no Lambda bandwidth limits

### 4. CloudRepoData Serialization
- **AST Stripping**: Removes unparseable/large AST nodes before upload
- **JSON Compression**: Native JSON serialization is compact
- **Selective Loading**: Can fetch metadata without downloading type files

### 5. TTL (Time-To-Live)
- **Setting**: 30 days on DynamoDB items
- **Benefit**: Automatic cleanup of stale repositories
- **Cost**: Reduces storage costs for inactive repos

### 6. Lambda Warm-Up
- **Cold Start**: First invocation ~1-2 seconds
- **Warm Invocations**: ~50-200ms after warm-up
- **Optimization**: Small deployment packages reduce cold start times

### 7. Health Check Caching
- **Strategy**: Rust client performs health check before operations
- **Fail-Fast**: Detects connectivity issues early
- **Implementation**: Sends minimal request, accepts 401/403 as "healthy"

### 8. Cross-Repo Data Batching
- **Strategy**: Single `get-cross-repo-data` call retrieves all repos
- **Benefit**: Reduces API Gateway invocations (cost savings)
- **Trade-off**: Larger response payloads (acceptable for <100 repos)

### 9. Adjacent Repo Metadata
- **Feature**: `check-or-upload` response includes `adjacent` array
- **Purpose**: Returns nearby repos in same org without separate call
- **Use Case**: Quick reference during upload phase

### 10. Gemini Proxy Rate Limiting
- **Daily Limit**: 2000 requests/day (in-memory counter)
- **Reset**: Midnight UTC
- **Headers**: `X-Daily-Remaining`, `X-Daily-Limit` for client awareness
- **Fail-Safe**: Prevents runaway API costs

---

## Purpose of Each Lambda Function

### check-or-upload Lambda

**Primary Responsibilities**:
1. **Check Existence**: Determine if type files already exist for a commit
2. **Generate Upload URLs**: Create pre-signed S3 URLs for new uploads
3. **Metadata Management**: Store and retrieve repository metadata
4. **Cross-Repo Discovery**: Return all repositories in an organization
5. **File Download Proxy**: Securely download S3 files through Lambda

**Why a Lambda?**:
- Centralized authentication (API keys managed in one place)
- S3 pre-signed URL generation requires AWS credentials
- DynamoDB access control (no direct database exposure)
- Request validation and sanitization

**API Actions**:
- `check-or-upload`: Smart detection of upload necessity
- `store-metadata`: Update metadata without file upload
- `complete-upload`: Two-phase commit for S3 + DynamoDB consistency
- `get-cross-repo-data`: Batch retrieval with pagination
- `download-file`: Secure S3 proxy

### gemini-proxy Lambda

**Primary Responsibilities**:
1. **API Key Protection**: Hide Google Gemini API key from clients
2. **Rate Limiting**: Enforce daily usage limits to control costs
3. **Request Validation**: Limit message size, count, and format
4. **Message Transformation**: Convert message formats for Gemini compatibility
5. **Error Handling**: Translate Gemini errors to user-friendly messages

**Why a Lambda?**:
- **Security**: Gemini API key never exposed to client
- **Cost Control**: Rate limiting prevents bill shock
- **Flexibility**: Can switch AI providers without client changes
- **Monitoring**: Centralized logging of AI usage

**Features**:
- Model: `gemini-2.5-flash` (fast, cost-effective)
- CORS support for web clients
- Daily limit: 2000 requests (configurable)
- Max message size: 5MB (handles large codebases)
- Response time tracking: `X-Response-Time` header

---

## Data Flow for Multi-Repo Analysis

### Complete End-to-End Flow

**Phase 1: Repository Analysis (Rust - Local/CI)**
```
Parse files → Extract endpoints/calls → Generate types → Collect dependencies
```

**Phase 2: Data Persistence (Only on main branch)**
```
Rust → Lambda (check-or-upload) → DynamoDB (check hash)
  ↓ (if new hash)
Rust → S3 (upload types.ts) → Lambda (complete-upload) → DynamoDB (store metadata)
```

**Phase 3: Cross-Repo Discovery (Every run)**
```
Rust → Lambda (get-cross-repo-data) → DynamoDB (scan org) → Lambda (return all repos)
```

**Phase 4: Multi-Repo Analysis (Rust)**
```
1. Combine current repo + all downloaded repos
2. Build unified Analyzer with:
   - All API endpoints from all repos
   - All API calls from all repos
   - All dependencies from all repos
3. Compare and detect:
   - Endpoint/call mismatches (API contract violations)
   - Type mismatches (incompatible request/response types)
   - Dependency conflicts (version incompatibilities)
   - Missing environment variables
   - Configuration issues
```

**Phase 5: Result Formatting**
```
Analyzer → Formatter (Markdown) → GitHub PR comment or local output
```

---

## Key Integration Points

### 1. Rust ↔ Lambda Communication
- **Protocol**: HTTPS with JSON payloads
- **Authentication**: Bearer token in Authorization header
- **Error Handling**: HTTP status codes mapped to StorageError enum
- **Retry Logic**: None currently (could be added)

### 2. Lambda ↔ DynamoDB
- **Client**: AWS SDK v3 for JavaScript
- **Operations**: GetItem, PutItem, ScanCommand with pagination
- **Consistency**: Default eventual consistency (acceptable for this use case)
- **Error Handling**: Graceful degradation, partial results on errors

### 3. Lambda ↔ S3
- **Direct Uploads**: Client uses pre-signed URL (Lambda not involved)
- **Validation**: Lambda calls HeadObject to verify upload before storing metadata
- **Downloads**: Lambda proxies GetObjectCommand to avoid exposing S3 directly

### 4. Analyzer ↔ CloudRepoData
- **Deserialization**: Serde JSON with proper field renaming
- **AST Handling**: Types stripped before upload, not needed for cross-repo analysis
- **Config Merging**: Multiple configs merged using temp files

---

## Security Architecture

1. **API Key Authentication**: All Lambda endpoints require valid Bearer token
2. **S3 Bucket Lockdown**: All public access blocked
3. **Pre-Signed URLs**: Time-limited (5 minutes), scoped to specific keys
4. **IAM Least Privilege**: Lambdas only have necessary permissions
5. **HTTPS Only**: TLS 1.2 minimum for all communication
6. **Key Rotation**: API keys stored in Terraform variables for easy rotation
7. **Gemini API Key**: Never exposed to clients, only in Lambda environment

---

## Cost Optimization

1. **Pay-Per-Request**: DynamoDB billing model scales to zero
2. **HTTP API**: 70% cheaper than REST API Gateway
3. **Small Lambda Packages**: Faster cold starts, lower memory usage
4. **Pre-Signed URLs**: No Lambda proxy bandwidth costs
5. **TTL**: Automatic cleanup reduces storage costs
6. **Rate Limiting**: Prevents runaway Gemini API costs
7. **Conditional Uploads**: Only upload on hash changes

---

## Monitoring and Observability

1. **CloudWatch Logs**: All Lambda invocations logged with request/response details
2. **Structured Logging**: JSON logs for easy querying
3. **Metrics**: Lambda invocation counts, durations, errors
4. **Alarms**: SNS email on excessive usage (>20 invocations/minute)
5. **Response Time Tracking**: Gemini proxy returns `X-Response-Time` header
6. **Usage Headers**: `X-Daily-Remaining` shows rate limit status

---

## Summary

This architecture provides a scalable, secure, and cost-effective solution for multi-repository dependency analysis with the following key features:

- **Intelligent Caching**: Hash-based uploads prevent redundant data transfers
- **Cross-Repo Awareness**: Organizations can share type definitions and dependency information
- **Security First**: API keys protected, pre-signed URLs, TLS everywhere
- **Cost Optimized**: Pay-per-request, conditional uploads, rate limiting
- **Developer Friendly**: Mock storage for local testing, trait-based design for flexibility
- **AI-Powered**: Gemini integration for intelligent code analysis and recommendations

The system enables developers to catch API contract violations, dependency conflicts, and configuration issues across multiple repositories before they reach production.
