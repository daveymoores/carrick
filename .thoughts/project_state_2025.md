# Carrick Project - Complete Technical Overview (2025)

**Last Updated**: November 15, 2025
**Purpose**: Comprehensive documentation of the Carrick project's current implementation, architecture, and capabilities for AI agents and developers.

---

## Executive Summary

Carrick is a **cross-repository API consistency analysis tool** written in Rust that detects API mismatches, type incompatibilities, and dependency conflicts across microservices architectures. It combines traditional static code analysis (using SWC for AST parsing) with AI-powered code understanding (Gemini 2.5 Flash) to identify issues before they reach production.

### Key Capabilities

1. **API Endpoint Detection**: Extracts REST API definitions from Express.js applications
2. **API Call Extraction**: Identifies HTTP calls (fetch, axios) including complex dynamic URLs via AI
3. **Cross-Repository Analysis**: Shares API definitions across repos to detect mismatches
4. **Type Checking**: Validates TypeScript request/response type compatibility
5. **Dependency Analysis**: Detects version conflicts across microservices
6. **CI/CD Integration**: Generates GitHub-compatible markdown reports

### Operating Modes

- **Local Mode**: Analyzes a single repository for internal consistency
- **CI Mode**: Performs cross-repository analysis by sharing data via AWS cloud storage

---

## Architecture Overview

### Technology Stack

- **Core Language**: Rust
- **Parser**: SWC (Speedy Web Compiler) for JavaScript/TypeScript
- **AI Model**: Google Gemini 2.5 Flash (via proxy Lambda)
- **Cloud Infrastructure**: AWS (API Gateway, Lambda, DynamoDB, S3)
- **Type Checking**: TypeScript Compiler API via ts-morph
- **Infrastructure as Code**: Terraform

### High-Level Flow

```
┌─────────────────────────────────────────────────────────────┐
│                     Carrick CLI (Rust)                       │
│                                                              │
│  1. Parse JS/TS files (SWC)                                 │
│  2. Extract endpoints & calls (AST + AI)                    │
│  3. Resolve TypeScript types                                │
│  4. Collect dependencies from package.json                  │
└──────────────────────┬──────────────────────────────────────┘
                       │
                       ▼
         ┌─────────────────────────────┐
         │  Branch Check (GitHub CI)    │
         └──┬────────────────────────┬──┘
            │                        │
       main │                        │ PR/feature branch
            │                        │
            ▼                        │
    ┌───────────────┐               │
    │ Upload to AWS │               │
    │  - DynamoDB    │               │
    │  - S3          │               │
    └───────────────┘               │
                                    │
            ┌───────────────────────┘
            │
            ▼
┌──────────────────────────────────────┐
│  Download Cross-Repo Data from AWS   │
└──────────────┬───────────────────────┘
               │
               ▼
┌──────────────────────────────────────┐
│      Build Combined Analyzer          │
│  - Merge all repo endpoints           │
│  - Merge all repo calls               │
│  - Merge all dependencies             │
└──────────────┬───────────────────────┘
               │
               ▼
┌──────────────────────────────────────┐
│         Run Analysis Checks           │
│  - Match endpoints to calls           │
│  - Check type compatibility           │
│  - Detect dependency conflicts        │
│  - Find missing environment vars      │
└──────────────┬───────────────────────┘
               │
               ▼
┌──────────────────────────────────────┐
│   Generate GitHub Markdown Report    │
└──────────────────────────────────────┘
```

---

## Core Components Deep Dive

### 1. Parser Layer (`src/parser.rs`)

**Purpose**: Wraps SWC parser to convert JavaScript/TypeScript files into Abstract Syntax Trees (ASTs).

**Key Features**:
- Auto-detects file type (`.ts`, `.tsx`, `.js`, `.jsx`)
- Creates `SourceMap` for tracking positions (used for type resolution)
- Returns `Module` AST for visitor processing

**File**: `src/parser.rs` (61 lines)

---

### 2. Visitor Layer (`src/visitor.rs`)

**Purpose**: Implements the Visitor pattern to traverse ASTs and extract meaningful data.

**What It Extracts**:

1. **Express Applications**
   - Detects: `const app = express()`
   - Tracks: App name, initialization location

2. **Express Routers**
   - Detects: `const router = express.Router()`
   - Tracks: Router name, parent app

3. **Route Definitions**
   - Detects: `app.get('/users', handler)`, `router.post('/comments', ...)`
   - Extracts: HTTP method, path, handler name, parameter names

4. **Router Mounts**
   - Detects: `app.use('/api', router)`
   - Tracks: Mount path, parent-child relationships

5. **Function Definitions**
   - Captures: Async functions, arrow functions, function declarations
   - Purpose: Sent to Gemini AI for HTTP call extraction

6. **Import/Export Relationships**
   - Tracks: Named imports, default imports, re-exports
   - Purpose: Resolves handler references across files

**Data Structure**:

```rust
pub struct DependencyVisitor {
    pub endpoints: Vec<Endpoint>,           // Route definitions
    pub calls: Vec<Call>,                   // HTTP calls
    pub mounts: Vec<Mount>,                 // Router mounts
    pub express_apps: HashMap<String, AppContext>,
    pub routers: HashMap<String, RouterContext>,
    pub function_definitions: HashMap<String, FunctionDefinition>,
    pub imported_symbols: HashMap<String, ImportedSymbol>,
}
```

**File**: `src/visitor.rs` (1034 lines)

---

### 3. AI Extraction Layer (`src/gemini_service.rs`)

**Purpose**: Uses Gemini 2.5 Flash to extract HTTP calls from complex JavaScript code that pattern matching cannot handle.

**Why AI is Needed**:

Traditional AST analysis fails for:

```javascript
// Template literals with environment variables
const url = `${process.env.API_URL}/users/${userId}`;

// Dynamic URL construction
const endpoint = BASE_URL + '/api' + path;

// Conditional URL building
const url = isProduction
  ? `https://api.prod.com${route}`
  : `http://localhost:3000${route}`;
```

**AI Prompt Strategy**:

The prompt (lines 104-167 in `gemini_service.rs`) instructs Gemini to:
1. Identify all HTTP calls (fetch, axios, request, etc.)
2. Normalize template literals (`` `${BASE_URL}/users` `` → `ENV_VAR:BASE_URL:/users`)
3. Convert parameter placeholders (`` `${userId}` `` → `:id`)
4. Extract TypeScript type annotations with precise positions
5. Return JSON-only output (no markdown)

**Flow**:

```
Function ASTs → Batch to Gemini Proxy → Parse JSON Response → Normalize URLs → Store as Calls
```

**Safety Features**:
- Size protection: Warns if payload >200KB
- Retry logic: 3 attempts with exponential backoff for 503 errors
- Emergency disable: `DISABLE_GEMINI` env var
- Mock mode: `CARRICK_MOCK_ALL` for testing

**Rate Limiting**: 2000 requests/day via Gemini proxy Lambda

**File**: `src/gemini_service.rs` (479 lines)

---

### 4. Analyzer Layer (`src/analyzer/mod.rs`)

**Purpose**: Core analysis engine that matches API endpoints with calls, performs type checking, and detects issues.

**Key Data Structure**:

```rust
pub struct Analyzer {
    pub endpoints: Vec<ApiEndpointDetails>,    // API definitions (providers)
    pub calls: Vec<ApiEndpointDetails>,        // API calls (consumers)
    fetch_calls: Vec<Call>,                    // AI-extracted calls
    pub mounts: Vec<Mount>,                    // Router mounts
    pub apps: HashMap<String, AppContext>,
    endpoint_router: Option<matchit::Router>,  // Fast path matching
    all_repo_packages: HashMap<String, Packages>, // Cross-repo dependencies
}
```

**Analysis Methods**:

1. **`analyze_functions_for_fetch_calls()`**
   - Sends async functions to Gemini for HTTP call extraction
   - Normalizes AI responses into structured `Call` objects

2. **`resolve_types_for_endpoints()`**
   - Links TypeScript type annotations to API endpoints
   - Stores both AST nodes and string representations

3. **`build_endpoint_router()`**
   - Creates `matchit` router for fast path matching
   - Handles parameterized routes (`:id`, `:userId`)

4. **`analyze_matches()`**
   - Matches API calls to endpoint definitions
   - Identifies missing endpoints (consumers with no provider)
   - Identifies orphaned endpoints (providers with no consumer)

5. **`compare_calls_to_endpoints()`**
   - Validates request/response body compatibility
   - Checks TypeScript type assignments

6. **`check_type_compatibility()`**
   - Runs TypeScript compiler checks across repos
   - Reports incompatible type definitions

7. **`analyze_dependencies()`**
   - Detects version conflicts (major, minor, patch)
   - Categorizes by severity (Critical/Warning/Info)

**Path Resolution Algorithm**:

Handles nested Express mounts:

```javascript
// app.js
app.use('/api', apiRouter);

// apiRouter.js
apiRouter.use('/v1', v1Router);

// v1Router.js
v1Router.get('/users', handler);

// Result: GET /api/v1/users
```

**File**: `src/analyzer/mod.rs` (1615 lines)

---

### 5. Cloud Storage Layer (`src/cloud_storage/`)

**Purpose**: Abstraction layer for storing and retrieving cross-repository data.

#### Trait Definition (`mod.rs`)

```rust
#[async_trait]
pub trait CloudStorage {
    async fn upload_repo_data(&self, org: &str, data: &CloudRepoData);
    async fn download_all_repo_data(&self, org: &str) -> Vec<CloudRepoData>;
    async fn download_type_file_content(&self, s3_url: &str) -> String;
    async fn health_check(&self) -> Result<()>;
}
```

#### CloudRepoData Structure

```rust
pub struct CloudRepoData {
    pub repo_name: String,
    pub endpoints: Vec<ApiEndpointDetails>,        // APIs this repo provides
    pub calls: Vec<ApiEndpointDetails>,            // APIs this repo calls
    pub mounts: Vec<Mount>,                        // Router mounts
    pub apps: HashMap<String, AppContext>,
    pub function_definitions: HashMap<String, FunctionDefinition>,
    pub config_json: Option<String>,               // carrick.json
    pub package_json: Option<String>,              // Raw package.json
    pub packages: Option<Packages>,                // Parsed dependencies
    pub commit_hash: String,
    pub last_updated: DateTime<Utc>,
}
```

**Important**: AST nodes (`request_type`, `response_type`) are stripped before upload to avoid serialization issues.

#### AWS Implementation (`aws_storage.rs`)

**Three-Phase Upload**:

1. **Check Phase**: `POST /types/check-or-upload`
   - Sends: `{ org, repo, hash, filename }`
   - Receives: Pre-signed S3 URL (if upload needed) OR existing S3 URL

2. **Upload Phase**: Direct S3 upload
   - Uses pre-signed URL (no Lambda proxy)
   - Uploads type file (`.ts`) with 5-minute expiration

3. **Complete Phase**: `POST /types/complete-upload`
   - Sends: `{ org, repo, hash, s3_url, cloudRepoData }`
   - Lambda validates S3 file exists, then stores metadata in DynamoDB

**Download Flow**:

- Single call: `POST /types/get-cross-repo-data` with `{ org }`
- Returns: Array of all repos in organization with full `CloudRepoData`
- Pagination: Handled automatically by Lambda for large orgs

**Authentication**: Bearer token via `CARRICK_API_KEY` environment variable

**File**: `src/cloud_storage/aws_storage.rs` (418 lines)

#### Mock Implementation (`mock_storage.rs`)

**Purpose**: Local testing without AWS dependencies

**Features**:
- In-memory `HashMap` storage
- Pre-configured repos with dependency conflicts for testing
- Activates via `CARRICK_MOCK_ALL=true` env var

**File**: `src/cloud_storage/mock_storage.rs`

---

### 6. Type Checking System (`ts_check/`)

**Purpose**: Validates TypeScript type compatibility between API producers and consumers across repositories.

**Architecture**:

```
┌──────────────────────────────────────────────────────────┐
│                  Rust Analyzer                            │
│  Extracts TypeScript type references with positions       │
└───────────────────────┬──────────────────────────────────┘
                        │
                        ▼
                ┌───────────────┐
                │  TypeInfo[]   │
                │  (JSON)       │
                └───────┬───────┘
                        │
                        ▼
┌───────────────────────────────────────────────────────────┐
│  extract-type-definitions.ts (TypeScript)                 │
│  - Uses ts-morph to parse TypeScript AST                  │
│  - Resolves types at specified positions                  │
│  - Collects transitive dependencies                       │
│  - Generates standalone type files                        │
└───────────────────────┬───────────────────────────────────┘
                        │
                        ▼
          ┌─────────────────────────────┐
          │  {repo}_types.ts files      │
          │  package.json               │
          └─────────────┬───────────────┘
                        │
                        ▼
┌───────────────────────────────────────────────────────────┐
│  run-type-checking.ts (TypeScript)                        │
│  - Installs npm dependencies                              │
│  - Loads all type files                                   │
│  - Matches producers with consumers                       │
│  - Runs TypeScript compiler checks                        │
│  - Reports compatibility results                          │
└───────────────────────┬───────────────────────────────────┘
                        │
                        ▼
          ┌─────────────────────────────┐
          │  type-check-results.json    │
          └─────────────────────────────┘
```

**Key Features**:

1. **Naming Convention**:
   - Producers: `{Method}{Endpoint}ResponseProducer`
   - Consumers: `{Method}{Endpoint}ResponseConsumerCall{N}`
   - Example: `GetApiUsersResponseProducer` vs `GetApiUsersResponseConsumerCall1`

2. **Path Normalization**:
   - Converts camelCase type names to HTTP endpoints
   - `GetApiUsersById` → `GET /api/users/:id`
   - Handles environment variable patterns: `ENV_VAR:API_URL:/users`

3. **Type Unwrapping**:
   - Automatically unwraps `Response<T>` wrappers
   - Compares inner types for compatibility

4. **TypeScript Diagnostics Integration**:
   - Creates temporary type assignments
   - Captures actual TypeScript compiler error messages
   - Provides detailed incompatibility explanations

**Files**:
- `ts_check/extract-type-definitions.ts` - Type extraction entry point
- `ts_check/run-type-checking.ts` - Type validation entry point
- `ts_check/lib/type-extractor.ts` - Extraction orchestration
- `ts_check/lib/type-checker.ts` - Compatibility validation (682 lines)
- `ts_check/lib/argument-parser.ts` - CLI argument parsing
- `ts_check/lib/types.ts` - Type definitions

---

### 7. Output Formatter (`src/formatter/mod.rs`)

**Purpose**: Generates GitHub-compatible markdown reports for CI/CD integration.

**Output Format**:

```markdown
<!-- CARRICK_OUTPUT_START -->
<!-- CARRICK_ISSUE_COUNT:24 -->
### 🪢 CARRICK: API Analysis Results

#### 🔴 Critical: API Mismatches (5)
<details>
<summary>View Details</summary>

| Issue | Details |
|-------|---------|
| Type mismatch | GET /api/users expects User[], got User |
...
</details>

#### ⚠️ Connectivity Issues (8)
<details>
<summary>Missing Endpoints (3)</summary>

| Method | Endpoint | Called From |
|--------|----------|-------------|
| GET | /api/missing | repo-a:handler.js:42 |
...
</details>

#### 📦 Dependency Conflicts (11)
- **Critical (2)**: express 5.0.0 (repo-a) vs 4.18.0 (repo-b)
- **Warning (5)**: react 18.3.0 vs 18.2.0
- **Info (4)**: lodash patch differences

#### 💡 Configuration Suggestions
Unknown environment variables: API_KEY_V2, DATABASE_URL
<!-- CARRICK_OUTPUT_END -->
```

**Categories**:

1. **Critical** - Type mismatches, method conflicts
2. **Connectivity** - Missing endpoints, orphaned definitions
3. **Dependencies** - Version conflicts by severity
4. **Configuration** - Missing or unknown environment variables

**File**: `src/formatter/mod.rs` (687 lines)

---

### 8. Configuration System (`src/config.rs`)

**Purpose**: Classifies API calls as internal (microservices) or external (third-party).

**Configuration File** (`carrick.json`):

```json
{
  "internalEnvVars": ["API_URL", "SERVICE_URL", "INTERNAL_API"],
  "externalEnvVars": ["STRIPE_API_KEY", "GITHUB_TOKEN"],
  "internalDomains": ["api.yourcompany.com", "*.internal"],
  "externalDomains": ["api.stripe.com", "api.github.com"]
}
```

**Why Important**: Carrick skips external API calls in analysis since they're not part of the microservices ecosystem being analyzed.

**File**: `src/config.rs` (151 lines)

---

### 9. Main Engine (`src/engine/mod.rs`)

**Purpose**: Orchestrates the entire analysis pipeline.

**Key Function**: `run_analysis_engine()`

**Flow**:

1. **Branch Detection**
   - Checks `GITHUB_REF` and `GITHUB_EVENT_NAME`
   - Determines if upload is allowed (main branch only)

2. **Current Repo Analysis**
   - Discovers files (`.js`, `.ts`, `.jsx`, `.tsx`)
   - Parses files with SWC
   - Runs visitor to extract data
   - Builds analyzer
   - Sends functions to Gemini for call extraction
   - Extracts TypeScript types

3. **Conditional Upload** (main branch only)
   - Strips AST nodes from data
   - Uploads to AWS via cloud storage trait

4. **Cross-Repo Data Download** (all branches)
   - Fetches all repos in organization
   - Filters out current repo

5. **Combined Analysis**
   - Builds unified analyzer with all repos
   - Downloads type files from S3
   - Recreates npm environment
   - Runs TypeScript compiler checks

6. **Result Generation**
   - Analyzes matches (missing/orphaned endpoints)
   - Checks type compatibility
   - Detects dependency conflicts
   - Formats as GitHub markdown

**File**: `src/engine/mod.rs` (788 lines)

---

## AWS Infrastructure

### Architecture Diagram

```
┌──────────────────────────────────────────────────────────┐
│                    API Gateway (HTTP v2)                  │
│                                                           │
│  POST /types/check-or-upload  →  check-or-upload Lambda  │
│  POST /gemini/chat            →  gemini-proxy Lambda     │
└──────────────────┬────────────────────┬──────────────────┘
                   │                    │
        ┌──────────▼──────────┐  ┌─────▼──────────┐
        │  check-or-upload    │  │  gemini-proxy   │
        │  Lambda             │  │  Lambda         │
        │  (Node.js 22.x)     │  │  (Node.js 22.x) │
        │  30s timeout        │  │  60s timeout    │
        └──────┬──────┬───────┘  └────────┬────────┘
               │      │                   │
       ┌───────▼──┐   │           ┌───────▼────────┐
       │ DynamoDB │   │           │  Gemini API    │
       │  Table   │   │           │  (Google)      │
       └──────────┘   │           └────────────────┘
                      │
               ┌──────▼──────┐
               │  S3 Bucket  │
               │ (Type files)│
               └─────────────┘
```

### Lambda: check-or-upload

**File**: `lambdas/check-or-upload/index.js` (499 lines)

**Actions**:

1. **`check-or-upload`** (lines 115-216)
   - Checks if type file exists for commit hash
   - Returns existing S3 URL if found
   - Generates pre-signed S3 upload URL if needed
   - Includes adjacent repos in response

2. **`store-metadata`** (lines 218-263)
   - Stores CloudRepoData in DynamoDB
   - Used when type file already exists (hash unchanged)

3. **`complete-upload`** (lines 350-432)
   - Validates S3 file upload succeeded (HeadObject)
   - Stores metadata in DynamoDB
   - Two-phase commit for consistency

4. **`get-cross-repo-data`** (lines 265-348)
   - Scans DynamoDB for all repos in organization
   - Handles pagination (LastEvaluatedKey)
   - Returns full CloudRepoData for each repo

5. **`download-file`** (lines 434-470)
   - Proxies S3 GetObject requests
   - Returns type file content as text

**DynamoDB Schema**:

```
Partition Key (pk): "repo#${org}/${repo}"
Sort Key (sk):      "types"
TTL:                30 days (auto-cleanup)
```

**S3 Key Pattern**: `${org}/${repo}/${commit_hash}/${filename}`

**Security**:
- API key validation (Authorization: Bearer token)
- S3 URL validation (bucket and pattern verification)
- Pre-signed URL expiration: 5 minutes

### Lambda: gemini-proxy

**File**: `lambdas/gemini-proxy/index.js` (346 lines)

**Purpose**: Rate-limited proxy for Gemini API calls with cost protection.

**Features**:
- Daily limit: 2000 requests (resets midnight UTC)
- Request size limit: 5MB
- Message limit: 10 messages per request
- Hardcoded model: `gemini-2.5-flash`
- CORS support for web clients

**Rate Limiting**:
- In-memory counter (resets on cold start)
- Headers: `X-Daily-Remaining`, `X-Daily-Limit`
- 429 status on quota exceeded

**Authentication**:
- Client: `CARRICK_API_KEY` (Bearer token)
- Gemini: `GEMINI_API_KEY` (stored in Lambda env vars)

### Terraform Configuration

**Files**: `terraform/*.tf`

**Key Resources**:

1. **`api_gateway.tf`** - HTTP API v2 with routes
2. **`lambda.tf`** - Function definitions and IAM roles
3. **`dynamodb.tf`** - Table with TTL enabled
4. **`s3.tf`** - Bucket with public access blocked
5. **`iam.tf`** - Least-privilege role definitions
6. **`cloudwatch.tf`** - Logging and alarms
7. **`custom_domain.tf`** - Optional domain configuration

**Deployment**:

```bash
cd terraform
terraform init
terraform plan
terraform apply
```

**Environment Variables** (set in Lambda):
- `CARRICK_API_KEY` - Authentication token
- `GEMINI_API_KEY` - Google AI API key
- `DYNAMODB_TABLE_NAME` - DynamoDB table name
- `S3_BUCKET_NAME` - S3 bucket name

---

## Data Models

### ApiEndpointDetails

**Location**: `src/analyzer/mod.rs:62-80`

```rust
pub struct ApiEndpointDetails {
    pub owner: Option<OwnerType>,         // App or Router
    pub route: String,                    // "/users/:id"
    pub method: String,                   // "GET"
    pub params: Vec<String>,              // ["id"]
    pub request_body: Option<Json>,       // JSON schema
    pub response_body: Option<Json>,      // JSON schema
    pub handler_name: Option<String>,     // "getUserById"
    pub request_type: Option<TypeReference>,  // TS type AST
    pub response_type: Option<TypeReference>, // TS type AST
    pub file_path: PathBuf,               // Source location
}
```

### TypeReference

**Location**: `src/visitor.rs:30-38`

```rust
pub struct TypeReference {
    pub file_path: PathBuf,
    pub type_ann: Option<Box<TsType>>,    // SWC AST node
    pub start_position: usize,             // UTF-16 offset
    pub composite_type_string: String,     // "User[]"
    pub alias: String,                     // "GetUsersResponse"
}
```

**Design Decision**: Stores both AST nodes (for local type checking) and string representations (for serialization).

### Mount

Represents Express router mounts (`app.use('/api', router)`):

```rust
pub struct Mount {
    pub path: String,              // "/api"
    pub router_name: String,       // "apiRouter"
    pub owner: String,             // "app"
    pub owner_type: OwnerType,     // App or Router
}
```

### DependencyConflict

**Location**: `src/analyzer/mod.rs`

```rust
pub struct DependencyConflict {
    pub package_name: String,               // "express"
    pub versions: Vec<(String, String)>,    // [("repo-a", "5.0.0"), ("repo-b", "4.18.0")]
    pub severity: ConflictSeverity,         // Critical/Warning/Info
    pub description: String,                // Human-readable explanation
}

pub enum ConflictSeverity {
    Critical,  // Major version difference
    Warning,   // Minor version difference
    Info,      // Patch version difference
}
```

---

## Key Algorithms

### 1. Path Resolution with Nested Mounts

**Problem**: Express allows nested router mounts that must be combined to get full paths.

**Example**:

```javascript
// server.js
const app = express();
app.use('/api', apiRouter);

// api.js
const apiRouter = express.Router();
apiRouter.use('/v1', v1Router);

// v1.js
const v1Router = express.Router();
v1Router.get('/users', handler);
```

**Algorithm** (`analyzer/path.rs`):

```rust
pub fn compute_full_paths_for_endpoint(
    endpoint: &ApiEndpointDetails,
    mounts: &[Mount],
    apps: &HashMap<String, AppContext>,
) -> Vec<String> {
    // 1. Start with endpoint route: "/users"
    // 2. Find all mounts targeting endpoint's owner (v1Router)
    // 3. Prepend mount path: "/v1/users"
    // 4. Find all mounts targeting that router's owner (apiRouter)
    // 5. Prepend mount path: "/api/v1/users"
    // 6. Continue until reaching root app
    // Result: ["/api/v1/users"]
}
```

### 2. AI Response Normalization

**Problem**: Gemini returns URLs with template literals that must be converted to Express patterns.

**Example**:

```javascript
// Original code:
const url = `${BASE_URL}/users/${userId}`;

// Gemini extracts:
"ENV_VAR:BASE_URL:/users/${userId}"

// Carrick normalizes to:
"ENV_VAR:BASE_URL:/users/:id"
```

**Algorithm** (`gemini_service.rs:normalize_url`):

1. Parse `ENV_VAR:VAR_NAME:/path` format
2. Find template literal placeholders: `${...}`
3. Convert to Express parameters: `:id`, `:userId`, etc.
4. Clean up extra slashes
5. Return normalized `Call` object

### 3. Type Compatibility Checking

**Problem**: Determine if consumer type is assignable from producer type.

**Algorithm** (`ts_check/lib/type-checker.ts:getTypeCompatibilityError`):

1. Create temporary TypeScript file with assignment:
   ```typescript
   const producer: ProducerType = {} as ProducerType;
   const consumer: ConsumerType = producer; // Test assignment
   ```

2. Run TypeScript compiler diagnostics

3. Capture error messages if assignment fails

4. Return detailed incompatibility explanation

**Why Effective**: Leverages TypeScript's own type system for accuracy.

### 4. Fuzzy Path Matching

**Problem**: Match API calls to endpoints even with different parameter names.

**Example**:
- Endpoint: `/users/:id`
- Call: `/users/:userId`
- Should match: ✅

**Algorithm** (`analyzer/mod.rs` using `matchit` router):

1. Build router with all endpoint patterns
2. For each call, attempt to match using `router.at()`
3. `matchit` handles parameter matching automatically
4. Extract matched parameters for validation

### 5. Dependency Conflict Detection

**Algorithm** (`analyzer/mod.rs:analyze_dependencies`):

```rust
pub fn analyze_dependencies(&self) -> Vec<DependencyConflict> {
    // 1. Collect all packages from all repos
    // 2. Group by package name
    // 3. For each package:
    //    a. Parse versions using semver
    //    b. Compare major/minor/patch numbers
    //    c. Determine severity:
    //       - Different major: Critical
    //       - Different minor: Warning
    //       - Different patch: Info
    // 4. Return list of conflicts
}
```

---

## Integration Points

### Environment Variables

**Required**:
- `CARRICK_ORG` - Organization name (e.g., "mycompany")
- `CARRICK_API_KEY` - Authentication token for AWS Lambda

**Optional**:
- `CARRICK_API_ENDPOINT` - API Gateway URL (compile-time constant)
- `CARRICK_MOCK_ALL` - Use mock storage (testing)
- `DISABLE_GEMINI` - Skip AI extraction
- `GEMINI_API_KEY` - Direct Gemini access (bypasses proxy)

**GitHub CI** (auto-detected):
- `GITHUB_REF` - Branch reference (refs/heads/main)
- `GITHUB_EVENT_NAME` - Event type (push, pull_request)

### CLI Usage

**Basic**:
```bash
carrick
```

**With Path**:
```bash
carrick /path/to/repo
```

**Exit Codes**:
- `0` - Success (analysis complete)
- `1` - Analysis found issues
- `2` - Fatal error (parse failures, network errors)

### GitHub Actions Integration

**Example** (`.github/workflows/carrick.yml`):

```yaml
name: API Analysis
on: [push, pull_request]

jobs:
  analyze:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install Carrick
        run: |
          curl -L https://github.com/yourorg/carrick/releases/latest/download/carrick-linux-x64 -o /usr/local/bin/carrick
          chmod +x /usr/local/bin/carrick

      - name: Run Analysis
        env:
          CARRICK_ORG: ${{ github.repository_owner }}
          CARRICK_API_KEY: ${{ secrets.CARRICK_API_KEY }}
        run: carrick

      - name: Comment PR
        if: github.event_name == 'pull_request'
        uses: actions/github-script@v6
        with:
          script: |
            const fs = require('fs');
            const output = fs.readFileSync('carrick_output.md', 'utf8');
            github.rest.issues.createComment({
              issue_number: context.issue.number,
              owner: context.repo.owner,
              repo: context.repo.repo,
              body: output
            });
```

---

## Testing

### Mock Storage Testing

**Activation**:
```bash
export CARRICK_MOCK_ALL=true
carrick
```

**What It Provides**:
- Pre-configured repos with known dependency conflicts
- In-memory storage (no network calls)
- Predictable test data

**Use Cases**:
- Development without AWS setup
- CI testing without cloud dependencies
- Feature development

### Gemini Bypass

**Disable AI Extraction**:
```bash
export DISABLE_GEMINI=true
carrick
```

**Effect**:
- Skips sending functions to Gemini
- Relies only on static AST analysis
- Faster execution, less accurate call detection

---

## Key Design Patterns

### 1. Trait-Based Abstraction

The `CloudStorage` trait allows swapping implementations:

```rust
// Production
let storage: Box<dyn CloudStorage> = Box::new(AwsStorage::new());

// Testing
let storage: Box<dyn CloudStorage> = Box::new(MockStorage::new());
```

**Benefit**: Testability without mocking infrastructure.

### 2. Builder Pattern

`Analyzer` construction uses builder pattern:

```rust
let analyzer = AnalyzerBuilder::new(repo_name)
    .build_from_visitors(visitors, source_maps)?;
```

**Benefit**: Clear separation between visitor data collection and analyzer construction.

### 3. Visitor Pattern

AST traversal uses SWC's built-in visitor:

```rust
impl Visit for DependencyVisitor {
    fn visit_call_expr(&mut self, node: &CallExpr) {
        // Extract route definitions
    }
}
```

**Benefit**: Clean separation of concerns, extensible for new patterns.

### 4. Two-Phase Commit

S3 upload uses two-phase commit:

1. **Phase 1**: Upload file to S3
2. **Phase 2**: Verify upload, then commit metadata to DynamoDB

**Benefit**: Ensures metadata only exists if file exists.

### 5. AST Stripping for Serialization

Before upload, AST nodes are removed:

```rust
fn strip_ast_nodes(mut data: CloudRepoData) -> CloudRepoData {
    for endpoint in &mut data.endpoints {
        endpoint.request_type = None;
        endpoint.response_type = None;
    }
    data
}
```

**Benefit**: Avoid serialization errors, reduce payload size.

---

## Performance Optimizations

### 1. Hash-Based Caching

- Only upload when commit hash changes
- Lambda checks DynamoDB before generating upload URL
- Avoids redundant uploads for unchanged code

### 2. Pre-Signed URLs

- Direct client-to-S3 upload (no Lambda proxy)
- 5-minute expiration limits exposure
- No Lambda bandwidth limits

### 3. Parallel File Parsing

Uses `rayon` for parallel file parsing:

```rust
files.par_iter().for_each(|file| {
    parse_file(file);
});
```

**Benefit**: Scales with CPU cores.

### 4. Matchit Router

Fast path matching with `matchit` crate:

```rust
let router = matchit::Router::new();
router.insert("/users/:id", endpoint);
router.at("/users/123"); // O(log n) lookup
```

**Benefit**: Faster than linear scan for large endpoint counts.

### 5. Incremental Type Checking

TypeScript type checking only runs when:
- Cross-repo data is available
- Type files downloaded from S3
- Consumer types reference producer types

**Benefit**: Avoids expensive type checks when not needed.

---

## Current Limitations & Known Issues

### 1. Naming Convention Dependency

Type checking relies on specific naming patterns:
- `{Method}{Endpoint}ResponseProducer`
- `{Method}{Endpoint}ResponseConsumerCall{N}`

**Impact**: Custom naming breaks type matching.

### 2. Position-Based Type Extraction

TypeScript type extraction requires precise UTF-16 offsets.

**Impact**: Position calculation must be exact; off-by-one errors cause failures.

### 3. Cold Start Latency

Lambda cold starts add 1-2 seconds to first request.

**Impact**: CI builds experience occasional slowness.

### 4. Rate Limiting

Gemini proxy has 2000 requests/day limit.

**Impact**: Large organizations may hit limits.

### 5. No Partial Updates

Entire repo data is uploaded/downloaded each time.

**Impact**: Network overhead for large repos.

### 6. Single Region

AWS infrastructure is single-region.

**Impact**: Latency for global teams.

---

## Comparison to agent_memo.md

### What Matches

✅ AWS infrastructure (DynamoDB + S3 + Lambda)
✅ Single Lambda handles multiple actions
✅ CloudRepoData structure as described
✅ AI extraction with Gemini 2.5 Flash
✅ Cross-repo analysis functional
✅ SWC-based parsing
✅ Type checking with TypeScript compiler

### What's Different

**agent_memo.md mentions**: `extracted_types: Vec<serde_json::Value>` in CloudRepoData
**Reality**: Type references are in `ApiEndpointDetails.request_type` and `response_type`

**agent_memo.md says**: "Recently completed" cloud migration
**Reality**: AWS infrastructure is mature and production-ready

**agent_memo.md lacks**:
- Gemini proxy Lambda details
- Formatter implementation details
- Path resolution algorithm
- Type compatibility checking flow
- Mock storage details

---

## Future Enhancement Opportunities

### 1. GraphQL Support

**Goal**: Analyze GraphQL schemas and resolvers

**Implementation**:
- Add GraphQL parser visitor
- Extract schema definitions
- Match resolvers to schema fields
- Validate resolver return types

### 2. WebSocket API Detection

**Goal**: Detect socket.io and ws connections

**Implementation**:
- Add visitor patterns for socket definitions
- Track event names and payloads
- Cross-repo socket event matching

### 3. Parallel Type Checking

**Goal**: Speed up TypeScript compiler checks

**Implementation**:
- Process type files in parallel
- Use worker threads for ts-morph operations
- Batch results

### 4. Incremental Analysis

**Goal**: Only analyze changed files

**Implementation**:
- Git diff detection
- Cached AST storage
- Partial visitor runs

### 5. API Versioning Support

**Goal**: Track API versions and breaking changes

**Implementation**:
- Version extraction from routes (/v1/, /v2/)
- Historical comparison
- Breaking change detection

### 6. Multi-Region Support

**Goal**: Reduce latency for global teams

**Implementation**:
- DynamoDB global tables
- Multi-region S3 replication
- Region-aware routing

### 7. Visualization Dashboard

**Goal**: Web UI for exploring API relationships

**Implementation**:
- Interactive graph visualization
- Dependency tree explorer
- Historical trend analysis

---

## Conclusion

Carrick is a **production-ready, sophisticated API analysis tool** that successfully combines:

1. **Static Analysis** (SWC) for fast, accurate AST traversal
2. **AI-Powered Extraction** (Gemini) for complex dynamic URL patterns
3. **Cross-Repository Intelligence** (AWS) for microservices coordination
4. **Type Safety Validation** (TypeScript Compiler) for contract enforcement
5. **CI/CD Integration** (GitHub Actions) for automated analysis

**Architecture Strengths**:
- Clean separation of concerns (Parser → Visitor → Analyzer → Formatter)
- Trait-based design for testability
- Two-phase commits for data consistency
- Rate limiting and cost protection
- Comprehensive error handling

**Production Readiness**:
- AWS infrastructure with Terraform IaC
- Mock storage for testing
- Detailed logging and monitoring
- Security best practices (least privilege, TLS, API keys)
- GitHub-compatible output formatting

This document serves as the **definitive technical reference** for understanding Carrick's current implementation, architecture, and capabilities.
