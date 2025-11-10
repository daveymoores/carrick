# Carrick Multi-Agent Architecture Research Document

## Executive Summary

**⚠️ CRITICAL: This is a migration-in-progress document for completing the framework-agnostic multi-agent architecture.**

Carrick is transitioning from a framework-specific (Express-focused) API analysis tool to a **framework-agnostic multi-agent system** that can analyze any JavaScript/TypeScript codebase regardless of the HTTP framework or library used. The system combines Rust-based AST analysis with LLM-powered semantic understanding and TypeScript-based cross-repository type checking to catch API mismatches before deployment.

**Key Innovation**: The new system uses a **Classify-Then-Dispatch** pattern where a lightweight triage agent classifies all code patterns first, then specialized agents extract detailed information only from relevant call sites. Cross-repo type compatibility is ensured through a sophisticated TypeScript type extraction and checking system.

### 🎯 Primary Goal

**REMOVE ALL LEGACY CODE** that makes the tool brittle and prevents it from being framework-agnostic. The multi-agent system (`src/multi_agent_orchestrator.rs`) is the future—legacy visitors and Express-specific code must be eliminated.

### ✅ What's Complete

- Framework-agnostic call site extraction (`src/call_site_extractor.rs`)
- Framework detection via LLM (`src/framework_detector.rs`)
- Classify-Then-Dispatch orchestration (`src/agents/orchestrator.rs`)
- Five specialist agents (Triage, Endpoint, Consumer, Mount, Middleware)
- Mount graph construction with behavior-based classification
- TypeScript type extraction and cross-repo checking (`ts_check/`)
- Structured output with Gemini schemas

### ❌ What's Missing (BLOCKERS)

1. **Type Extraction Integration**: Currently only extracts types from Gemini calls, not from multi-agent results
2. **Complete Legacy Removal**: Old visitor patterns still exist in `src/visitor.rs` and parts of `src/analyzer/`
3. **Direct Agent-to-CloudRepoData Flow**: Still using adapter pattern to convert to legacy format
4. **Agent-Based Type Extraction**: Need dedicated TypeExtractionAgent instead of Gemini-only
5. **Remove Framework-Specific Code**: All Express/router pattern matching must be deleted

---

## Table of Contents

1. [Expected Output Format](#expected-output-format)
2. [What's Missing to Complete This Feature](#whats-missing-to-complete-this-feature)
3. [Architecture Overview](#architecture-overview)
4. [Multi-Agent Workflow](#multi-agent-workflow)
5. [TypeScript Type Checking System (ts_check/)](#typescript-type-checking-system-ts_check)
6. [Core Components](#core-components)
7. [Framework-Agnostic Design Principles](#framework-agnostic-design-principles)
8. [Data Flow](#data-flow)
9. [Agent Responsibilities](#agent-responsibilities)
10. [Technical Implementation Details](#technical-implementation-details)
11. [Cross-Repository Analysis](#cross-repository-analysis)
12. [Legacy Code to Remove](#legacy-code-to-remove)
13. [LLM Optimization Strategies](#llm-optimization-strategies)

---

## 1. Expected Output Format

**File**: `src/formatter/mod.rs`

### 1.1 Output Structure

Carrick produces GitHub-flavored Markdown with collapsible sections, designed for PR comments and CI output.

**Format**:
```markdown
<!-- CARRICK_OUTPUT_START -->
<!-- CARRICK_ISSUE_COUNT:24 -->
### 🪢 CARRICK: API Analysis Results

Analyzed **20 endpoints** and **13 API calls** across all repositories.

Found **24 total issues**: **2 critical mismatches**, **17 connectivity issues**, **2 dependency conflicts**, and **3 configuration suggestions**.

<details>
  <summary><strong>2 Critical: API Mismatches</strong></summary>
  <!-- Critical issues here -->
</details>

<details>
  <summary><strong>17 Connectivity Issues</strong></summary>
  <!-- Missing/orphaned endpoints -->
</details>

<details>
  <summary><strong>2 Dependency Conflicts</strong></summary>
  <!-- Version conflicts -->
</details>

<details>
  <summary><strong>3 Configuration Suggestions</strong></summary>
  <!-- Environment variable usage -->
</details>
<!-- CARRICK_OUTPUT_END -->
```

### 1.2 Issue Categories

#### Critical Issues (API Mismatches)

**Source**: `issues.mismatches` + `issues.type_mismatches` + method mismatches from `issues.call_issues`

**Types**:
1. **Type Compatibility Issues**: TypeScript compiler finds type mismatches
   - Example: `GET /users/:id: Type '{ commentsByUser: Comment[]; }' missing properties from type 'User': id, name, role`
   - Formatted with endpoint, producer type, consumer type, and error details

2. **Method Mismatches**: HTTP method conflicts
   - Example: "Method mismatch: GET /orders called but endpoint only supports POST"

3. **Request Body Mismatches**: Request payload incompatibilities

**Display**: Groups similar issues, shows first occurrence with full details

#### Connectivity Issues

**Source**: `issues.endpoint_issues` + `issues.call_issues` (non-method-mismatch)

**Types**:
1. **Missing Endpoints**: API calls with no matching producer
   - Displayed as table: `| Method | Path |`
   - Example: `GET | ENV_VAR:ORDER_SERVICE_URL:/route-does-not-exist`

2. **Orphaned Endpoints**: Defined endpoints with no consumers
   - Displayed as table: `| Method | Path |`
   - Example: `GET | /api/orders`

**Purpose**: Identify dead code or misconfigured routes

#### Dependency Conflicts

**Source**: `issues.dependency_conflicts`

**Severity Levels**:
1. **Critical**: Major version differences (e.g., express 4.x vs 3.x)
2. **Warning**: Minor version differences (e.g., @types/node 18.15.0 vs 18.11.9)
3. **Info**: Patch version differences (typically low risk)

**Display**: Table format with repository, version, source path

```markdown
#### express

| Repository | Version | Source |
| :--- | :--- | :--- |
| `user-service` | `4.18.0` | `package.json` |
| `comment-service` | `3.4.8` | `package.json` |
```

#### Configuration Suggestions

**Source**: `issues.env_var_calls`

**Purpose**: Identify API calls using environment variables for URLs

**Display**:
- `GET` using **[COMMENT_SERVICE_URL]** in `/api/comments`
- `POST` using **[ORDER_SERVICE_URL]** in `/orders`

**Recommendation**: Add to `carrick.json` for better analysis:
```json
{
  "internalEnvVars": ["API_URL", "SERVICE_URL"],
  "externalEnvVars": ["STRIPE_API", "GITHUB_API"]
}
```

### 1.3 No Issues Output

When no issues found:
```markdown
<!-- CARRICK_OUTPUT_START -->
<!-- CARRICK_ISSUE_COUNT:0 -->
### 🪢 CARRICK: API Analysis Results

Analyzed **X endpoints** and **Y API calls** across all repositories.

✅ **No API inconsistencies detected!**

<!-- CARRICK_OUTPUT_END -->
```

---

## 2. What's Missing to Complete This Feature

### 🚨 CRITICAL: Framework Agnosticism Goal

**DO NOT support or maintain legacy code.** The goal is to **REMOVE ALL FRAMEWORK-SPECIFIC PATTERNS** that make the tool brittle. The multi-agent system already works—we need to eliminate the old code that prevents full framework agnosticism.

### 2.1 Type Extraction from Multi-Agent Results

**Current Problem**: Types are only extracted from Gemini-analyzed async calls, not from multi-agent results.

**What's Needed**:
1. Extract types from `EndpointAgent` results (HTTP endpoint handlers)
2. Extract types from `ConsumerAgent` results (API call sites)
3. Create unified type extraction that works with agent output, not just Gemini

**Implementation Path**:
```rust
// In src/multi_agent_orchestrator.rs
impl MultiAgentOrchestrator {
    // NEW: Extract types from agent results
    fn extract_types_from_analysis(
        &self,
        analysis_results: &AnalysisResults,
    ) -> Vec<TypeInfo> {
        let mut type_infos = Vec::new();
        
        // Extract from endpoints
        for endpoint in &analysis_results.endpoints {
            // Parse handler to find return types
            // Use ts-morph or AST traversal
        }
        
        // Extract from data calls
        for call in &analysis_results.data_fetching_calls {
            // Parse call site to find expected types
        }
        
        type_infos
    }
}
```

**Files to Modify**:
- `src/multi_agent_orchestrator.rs`: Add type extraction
- `src/agents/endpoint_agent.rs`: Include type info in results
- `src/agents/consumer_agent.rs`: Include type info in results

### 2.2 Remove Legacy Visitor Code

**Target for Deletion**:
- `src/visitor.rs`: Old Express-specific visitors (DependencyVisitor uses framework patterns)
- Parts of `src/analyzer/mod.rs`: Methods that use old visitor patterns
- `src/extractor.rs`: Framework-specific extraction logic

**Keep**:
- `src/parser.rs`: SWC parsing (framework-agnostic)
- `src/call_site_extractor.rs`: Universal call site extraction (already framework-agnostic)

**Action Items**:
1. Identify all uses of `DependencyVisitor` and replace with multi-agent flow
2. Remove Express-specific pattern matching (e.g., `express.Router()` checks)
3. Delete unused visitor traits and implementations

### 2.3 Direct CloudRepoData Construction

**Current Problem**: Using adapter pattern to convert multi-agent results to legacy `ApiEndpointDetails` format.

**What's Needed**: Build `CloudRepoData` directly from multi-agent results.

**Implementation**:
```rust
// In src/engine/mod.rs
impl CloudRepoData {
    pub fn from_multi_agent_results(
        analysis_result: &MultiAgentAnalysisResult,
        repo_name: String,
        config: Config,
        packages: Packages,
    ) -> Self {
        let mount_graph = &analysis_result.mount_graph;
        
        CloudRepoData {
            repo_name,
            endpoints: mount_graph.get_resolved_endpoints(), // Already correct format
            calls: mount_graph.get_data_calls(),
            mounts: mount_graph.get_mounts(),
            // ... directly populate fields
        }
    }
}
```

**Remove**: `convert_orchestrator_results_to_analyzer_data()` adapter function

### 2.4 Agent-Based Type Extraction

**Create**: `TypeExtractionAgent` that works with call sites directly.

**Why**: Currently relying on Gemini to extract types from async functions. Need systematic type extraction from ALL call sites.

**Design**:
```rust
// src/agents/type_extraction_agent.rs
pub struct TypeExtractionAgent {
    gemini_service: GeminiService,
}

impl TypeExtractionAgent {
    pub async fn extract_types(
        &self,
        endpoints: &[HttpEndpoint],
        data_calls: &[DataFetchingCall],
    ) -> Result<Vec<TypeInfo>, Box<dyn std::error::Error>> {
        // Extract types from endpoint handlers
        // Extract types from call sites
        // Return TypeInfo for ts_check
    }
}
```

### 2.5 Remove Framework-Specific Code Completely

**Search and Destroy**:
1. Any code checking for `express`, `Router()`, `app.get()`, etc.
2. Pattern matching on framework-specific function names
3. Hardcoded route parsing logic

**Replace With**: Let LLM agents identify patterns, use behavior-based classification only.

**Validation**: Tool should work identically with Express, Fastify, Koa, Hapi, NestJS, or custom frameworks.

### 2.6 Testing Requirements

**Must Have**:
1. Integration tests with multiple frameworks (Express, Fastify, Koa)
2. Tests confirming legacy code removal doesn't break analysis
3. Performance benchmarks (LLM usage, execution time)
4. Test fixtures for each framework type

**Test Strategy**:
- Use `tests/fixtures/` with sample apps from different frameworks
- Validate same issues detected regardless of framework
- Ensure type checking still works after migration

---

## 3. Architecture Overview

### 1.1 High-Level Design

The multi-agent architecture follows a **staged pipeline approach** with integrated TypeScript type checking:

```
Stage 0: Framework Detection
    ↓
Stage 1: Universal Call Site Extraction
    ↓
Stage 2: Classify-Then-Dispatch (Triage → Specialist Agents)
    ↓
Stage 3: Mount Graph Construction
    ↓
Stage 4: Type Extraction & Cross-Repo Type Checking
    ↓
Result: Framework-Agnostic API Analysis with Type Safety
```

### 1.2 Core Philosophy

**From**: Pattern-matching specific frameworks (Express, Fastify, etc.)
**To**: Understanding code behavior through LLM-powered semantic analysis + TypeScript compiler validation

**Key Principles**:
1. Don't assume framework semantics—let the LLM identify what code *does*
2. Extract types from actual usage sites for accurate compatibility checking
3. Use TypeScript's own compiler to validate type compatibility

---

## 2. Multi-Agent Workflow

### 2.1 Stage 0: Framework Detection

**File**: `src/framework_detector.rs`

**Purpose**: Identify which frameworks and libraries are actually used in the codebase.

**Input**:
- `package.json` dependencies and devDependencies
- Import statements extracted from source files

**Output**:
```rust
pub struct DetectionResult {
    pub frameworks: Vec<String>,      // e.g., ["express", "fastify"]
    pub data_fetchers: Vec<String>,   // e.g., ["axios", "fetch"]
    pub notes: String,                // Additional context
}
```

**LLM Role**: Classifies packages and imports into:
- HTTP frameworks (Express, Koa, Fastify, Hapi, NestJS, etc.)
- Data-fetching libraries (axios, fetch, got, superagent, graphql-request, etc.)

---

### 2.2 Stage 1: Universal Call Site Extraction

**File**: `src/call_site_extractor.rs`

**Purpose**: Extract ALL member call expressions from the codebase without filtering.

**Key Innovation**: Framework-agnostic—extracts `object.property(args)` patterns universally.

**Data Structure**:
```rust
pub struct CallSite {
    pub callee_object: String,        // e.g., "app", "router", "axios"
    pub callee_property: String,      // e.g., "get", "post", "use"
    pub args: Vec<CallArgument>,      // Extracted arguments
    pub definition: Option<String>,   // Variable definition context
    pub location: String,             // File:line:column
}
```

---

### 2.3 Stage 2: Classify-Then-Dispatch

**File**: `src/agents/orchestrator.rs`

#### Phase 1: Triage (Classification)

**File**: `src/agents/triage_agent.rs`

**Categories**:
```rust
pub enum TriageClassification {
    HttpEndpoint,       // Route definitions
    DataFetchingCall,   // Outbound API calls
    Middleware,         // Middleware registration
    RouterMount,        // Router mounting
    Irrelevant,         // Everything else
}
```

**Optimization**: Uses `LeanCallSite` (30-50% smaller) for classification.

#### Phase 2: Dispatch to Specialist Agents

- **EndpointAgent**: Extract HTTP endpoints (method, path, handler)
- **ConsumerAgent**: Extract outbound API calls (library, URL, method)
- **MountAgent**: Extract router mount relationships
- **MiddlewareAgent**: Extract middleware registrations

All run in parallel using `tokio::try_join!`.

---

### 2.4 Stage 3: Mount Graph Construction

**File**: `src/mount_graph.rs`

**Purpose**: Build a graph representation of how routers and endpoints are organized.

**Key Features**:
- **Behavior-based classification**: Nodes classified by mount behavior, not framework patterns
- **Path resolution**: Compute full paths by walking mount chain
- **Framework-agnostic**: Works with any routing pattern

---

### 2.5 Stage 4: Type Extraction & Cross-Repo Type Checking

**Critical Component**: This is where Carrick ensures type safety across repositories.

**Files**: `ts_check/` directory (TypeScript-based)

**Flow**:
1. Extract type references from API endpoints and calls
2. Generate standalone TypeScript files per repository
3. Share type files via cloud storage (S3)
4. Run TypeScript compiler on combined types
5. Report type compatibility issues

---

## 3. TypeScript Type Checking System (ts_check/)

### 3.1 Overview

The `ts_check/` directory contains a **TypeScript-based type extraction and checking system** that runs **after** the Rust analysis to validate type compatibility across repositories.

**Why TypeScript for Type Checking?**
- Native understanding of TypeScript types
- Can resolve complex types, generics, utility types
- Uses actual TypeScript compiler for validation
- Provides accurate error messages

---

### 3.2 Architecture

```
ts_check/
├── extract-type-definitions.ts   # Main extraction script
├── run-type-checking.ts          # Main type checking script
├── lib/
│   ├── type-extractor.ts         # Orchestrates type extraction
│   ├── type-checker.ts           # TypeScript-based type compatibility checking
│   ├── type-resolver.ts          # Resolves type references
│   ├── declaration-collector.ts  # Collects all dependent declarations
│   ├── dependency-manager.ts     # Manages npm dependencies
│   ├── import-handler.ts         # Handles import statements
│   ├── output-generator.ts       # Generates output files
│   └── ...
└── output/                       # Generated type files and results
    ├── repo-a_types.ts
    ├── repo-b_types.ts
    ├── package.json
    ├── tsconfig.json
    └── type-check-results.json
```

---

### 3.3 Type Extraction Process

#### 3.3.1 Entry Point: `extract-type-definitions.ts`

**Called by**: Rust code via `npx ts-node`

**Purpose**: Extract all type definitions needed for a single type reference.

**Input** (from Rust):
```json
[
  {
    "filePath": "src/api.ts",
    "startPosition": 1234,
    "compositeTypeString": "Response<User[]>",
    "alias": "GetUsersResponseProducerRepo1"
  }
]
```

**Process**:
1. Load project with `ts-morph`
2. Find type at position in source file
3. Recursively collect all dependent types
4. Handle utility types (Response<T>, Pick<T>, etc.)
5. Extract npm dependencies used by types
6. Generate standalone `.ts` file

**Output**: `ts_check/output/repo-a_types.ts`

---

#### 3.3.2 TypeExtractor Class

**File**: `ts_check/lib/type-extractor.ts`

**Key Methods**:
```typescript
async extractTypes(typeInfos: TypeInfo[], outputFile: string) {
  // For each type info:
  // 1. Find type reference at position
  // 2. Process type reference (collect dependencies)
  // 3. Add composite alias if needed
  // 4. Collect utility types
  // 5. Extract npm dependencies
  // 6. Generate output file
}
```

**Delegates to**:
- `TypeResolver`: Find type at specific position
- `DeclarationCollector`: Recursively collect all type dependencies
- `DependencyManager`: Track npm packages needed
- `ImportHandler`: Generate import statements
- `OutputGenerator`: Write final `.ts` file

---

#### 3.3.3 Declaration Collection

**File**: `ts_check/lib/declaration-collector.ts`

**Purpose**: Recursively collect all types referenced by a root type.

**Example**:
```typescript
// Root type
type GetUsersResponse = Response<User[]>;

// Collector finds:
// - Response<T> (utility type)
// - User (interface)
// - All properties of User (e.g., Address)
// - All types referenced by those properties
// - etc.
```

**Result**: Complete, self-contained type definition file.

---

### 3.4 Type Checking Process

#### 3.4.1 Entry Point: `run-type-checking.ts`

**Called by**: Rust code after all type files are collected

**Purpose**: Run TypeScript compiler on all repository type files to find incompatibilities.

**Process**:
1. Install dependencies in `ts_check/output/` (`npm install`)
2. Load all `*_types.ts` files with `ts-morph`
3. Extract producer and consumer types
4. Compare types for compatibility
5. Write results to `type-check-results.json`

**Output**:
```json
{
  "mismatches": [
    {
      "endpoint": "GET /users/:id",
      "producerType": "{ id: number; name: string; }",
      "consumerType": "User",
      "error": "Property 'role' is missing in producer type",
      "isCompatible": false
    }
  ],
  "compatibleCount": 15,
  "totalChecked": 16
}
```

---

#### 3.4.2 TypeCompatibilityChecker Class

**File**: `ts_check/lib/type-checker.ts`

**Key Responsibilities**:

1. **Parse Type Names**: Extract endpoint from generated type aliases
   ```typescript
   parseTypeName("GetApiCommentsResponseProducer")
   // -> { endpoint: "GET /api/comments", type: "producer" }
   
   parseTypeName("GetApiCommentsResponseConsumerCall1")
   // -> { endpoint: "GET /api/comments", type: "consumer", callId: "Call1" }
   ```

2. **Group Types**: Organize by endpoint
   ```typescript
   groupTypesByEndpoint()
   // -> {
   //   producers: Map<endpoint, producer_type>,
   //   consumers: Map<endpoint, consumer_type[]>
   // }
   ```

3. **Type Comparison**: Use TypeScript's type system
   ```typescript
   async compareTypes(endpoint, producer, consumer) {
     const producerType = producer.node.getType();
     const consumerType = consumer.node.getType();
     
     // Unwrap Response<T> wrapper
     producerType = this.unwrapResponseType(producerType);
     
     // Check assignability
     const isAssignable = producerType.isAssignableTo(consumerType);
     
     if (!isAssignable) {
       return {
         endpoint,
         producerType: producerType.getText(),
         consumerType: consumerType.getText(),
         errorDetails: this.getTypeCompatibilityError(...)
       };
     }
   }
   ```

4. **Get TypeScript Diagnostics**: Create test assignment to get error
   ```typescript
   getTypeCompatibilityError(producerType, consumerType) {
     // Create temporary file with test assignment
     const testCode = `
       declare const producer: ${producerType.getText()};
       const test: ${consumerType.getText()} = producer;
     `;
     
     // Get TypeScript's diagnostic message
     const diagnostics = tempFile.getPreEmitDiagnostics();
     return diagnostics.find(d => 
       d.getMessageText().includes("not assignable")
     );
   }
   ```

---

### 3.5 Type Naming Convention

**Critical for Matching**: Type aliases follow a strict naming convention.

**Format**:
- **Producer**: `{Method}{Path}Response/RequestProducer{RepoName}`
  - Example: `GetApiCommentsResponseProducerRepoA`
  
- **Consumer**: `{Method}{Path}Response/RequestConsumer{CallId}{RepoName}`
  - Example: `GetApiCommentsResponseConsumerCall1RepoB`

**Path Encoding**:
- `/api/comments` → `ApiComments`
- `/users/:id` → `UsersById`
- Environment variables: `ENV_VAR:ORDER_SERVICE_URL:/api` → `GetEnvVarOrderServiceUrlApi`

---

### 3.6 Integration with Rust

#### 3.6.1 Type Extraction Call

**File**: `src/analyzer/mod.rs` → `extract_types_for_repo()`

**Process**:
```rust
pub fn extract_types_for_repo(&self, repo_path: &str, ...) {
    // 1. Collect type infos from Gemini-extracted calls
    let type_infos = self.collect_type_infos_from_calls(&calls);
    
    // 2. Prepare JSON input
    let json_input = serde_json::to_string(&type_infos).unwrap();
    let output_path = format!("ts_check/output/{}_types.ts", repo_name);
    
    // 3. Run TypeScript extraction script
    Command::new("npx")
        .arg("ts-node")
        .arg("ts_check/extract-type-definitions.ts")
        .arg(&json_input)          // Type infos
        .arg(&output_path)         // Output file
        .arg(tsconfig_path)        // Repo's tsconfig
        .arg(&dependencies_json)   // NPM dependencies
        .output()
        .expect("Failed to run type extraction");
}
```

---

#### 3.6.2 Cross-Repo Type Checking

**File**: `src/engine/mod.rs` → `build_cross_repo_analyzer()`

**Process**:
```rust
async fn build_cross_repo_analyzer(...) {
    // 1. Download type files from all repos via S3
    recreate_type_files_and_check(&all_repo_data, &repo_s3_urls, storage).await?;
    
    // 2. Run final type checking across all repos
    analyzer.run_final_type_checking()?;
    
    // 3. Read results from type-check-results.json
    let type_check_result = analyzer.check_type_compatibility()?;
}
```

**Recreate Type Files** (`recreate_type_files_and_check`):
```rust
async fn recreate_type_files_and_check(...) {
    // 1. Clean output directory
    std::fs::remove_dir_all("ts_check/output")?;
    std::fs::create_dir_all("ts_check/output")?;
    
    // 2. Download type files from S3 for each repo
    for repo_data in all_repo_data {
        if let Some(s3_url) = repo_s3_urls.get(&repo_data.repo_name) {
            let content = storage.download_type_file_content(s3_url).await?;
            let file_path = format!("ts_check/output/{}_types.ts", repo_name);
            std::fs::write(&file_path, content)?;
        }
    }
    
    // 3. Create package.json with all dependencies
    recreate_package_and_tsconfig(output_dir, packages)?;
}
```

**Run Type Checking** (`run_final_type_checking`):
```rust
pub fn run_final_type_checking(&self) -> Result<(), String> {
    // Run the type checking script
    Command::new("npx")
        .arg("ts-node")
        .arg("ts_check/run-type-checking.ts")
        .arg(tsconfig_path)
        .output()
        .map_err(|e| format!("Failed to run type checking: {}", e))?;
    
    // Results are written to ts_check/output/type-check-results.json
    Ok(())
}
```

---

### 3.7 Type Check Results Flow

```
1. Rust extracts API calls with type references
   ↓
2. Calls ts_check/extract-type-definitions.ts for each repo
   ↓
3. TypeScript generates repo_name_types.ts files
   ↓
4. Files uploaded to S3 (DynamoDB metadata)
   ↓
5. In CI, all repos download type files from S3
   ↓
6. Calls ts_check/run-type-checking.ts
   ↓
7. TypeScript loads all type files, compares types
   ↓
8. Writes type-check-results.json
   ↓
9. Rust reads results and includes in final analysis
```

---

## 4. Core Components

### 4.1 MultiAgentOrchestrator

**File**: `src/multi_agent_orchestrator.rs`

**Role**: Top-level coordinator for the complete analysis workflow.

**Key Methods**:
```rust
pub async fn run_complete_analysis(
    &self,
    files: Vec<PathBuf>,
    packages: &Packages,
    imported_symbols: &HashMap<String, ImportedSymbol>,
) -> Result<MultiAgentAnalysisResult, Box<dyn std::error::Error>> {
    // Stage 0: Framework Detection
    let framework_detection = framework_detector
        .detect_frameworks_and_libraries(packages, imported_symbols)
        .await?;
    
    // Stage 1: Call Site Extraction
    let call_sites = self.extract_all_call_sites(&files).await?;
    
    // Stage 2: Classify-Then-Dispatch
    let orchestrator = CallSiteOrchestrator::new(self.gemini_service.clone());
    let analysis_results = orchestrator
        .analyze_call_sites(&call_sites, &framework_detection)
        .await?;
    
    // Stage 3: Mount Graph Construction
    let mount_graph = MountGraph::build_from_analysis_results(&analysis_results);
    
    Ok(MultiAgentAnalysisResult {
        framework_detection,
        mount_graph,
        analysis_results,
    })
}
```

---

### 4.2 Analyzer

**File**: `src/analyzer/mod.rs`

**Role**: Legacy analyzer now populated with multi-agent results + type checking.

**Type-Related Methods**:

```rust
// Collect type information from Gemini-extracted calls
pub fn collect_type_infos_from_calls(&self, calls: &[Call]) -> Vec<serde_json::Value>

// Extract types for current repository
pub fn extract_types_for_repo(&self, repo_path: &str, type_infos: Vec<Value>, packages: &Packages)

// Run final type checking across all repos
pub fn run_final_type_checking(&self) -> Result<(), String>

// Read type check results
pub fn check_type_compatibility(&self) -> Result<serde_json::Value, String>

// Get type mismatches for reporting
pub fn get_type_mismatches(&self) -> Vec<TypeMismatch>
```

---

### 4.3 CloudStorage Trait

**File**: `src/cloud_storage/mod.rs`

**Type-Related Methods**:

```rust
pub trait CloudStorage {
    // Upload type file to S3
    async fn upload_type_file(&self, repo_name: &str, type_content: &str) -> Result<String, String>;
    
    // Download type file from S3
    async fn download_type_file_content(&self, s3_url: &str) -> Result<String, String>;
    
    // Upload repo metadata (includes S3 URL for type file)
    async fn upload_repo_data(&self, org: &str, data: &CloudRepoData) -> Result<(), String>;
    
    // Download all repo data (includes S3 URLs)
    async fn download_all_repo_data(&self, org: &str) 
        -> Result<(Vec<CloudRepoData>, HashMap<String, String>), String>;
}
```

**AWS Implementation**: `src/cloud_storage/aws_storage.rs`
- Stores type files in S3
- Stores metadata in DynamoDB (includes S3 URL)
- Downloads type files for cross-repo analysis

---

## 5. Framework-Agnostic Design Principles

### 5.1 Behavioral Classification Over Pattern Matching

**Old Approach**: 
```rust
if definition.contains("express.Router()") {
    node_type = NodeType::Router;
}
```

**New Approach**:
```rust
// Classify based on mount behavior
if mounted_nodes.contains(&node_name) {
    node_type = NodeType::Mountable;
} else if parent_nodes.contains(&node_name) {
    node_type = NodeType::Root;
}
```

---

### 5.2 Universal Call Site Extraction

**Principle**: Extract all `object.property()` patterns without filtering.

**Why**: Let the LLM decide what matters, not hardcoded patterns.

---

### 5.3 LLM-Powered Semantic Understanding

**Principle**: Use LLM context to understand code semantics.

**Example**: Framework context informs classification without hardcoding framework-specific logic.

---

### 5.4 TypeScript Compiler for Type Safety

**Principle**: Use TypeScript's own type system for validation.

**Why**:
- More accurate than string matching
- Handles complex types, generics, utility types
- Provides detailed error messages
- Framework-agnostic (works with any TypeScript code)

---

## 6. Data Flow

### 6.1 End-to-End Pipeline

```
1. Parse Files (SWC)
   ↓
2. Extract All Call Sites (CallSiteExtractor)
   ↓
3. Detect Frameworks (FrameworkDetector) → LLM
   ↓
4. Triage Call Sites (TriageAgent) → LLM
   ↓
5. Dispatch to Specialist Agents (Parallel) → LLM
   ├─ EndpointAgent
   ├─ ConsumerAgent
   ├─ MiddlewareAgent
   └─ MountAgent
   ↓
6. Build Mount Graph (MountGraph)
   ↓
7. Extract Type References from Gemini Calls
   ↓
8. Generate TypeScript Type Files (ts_check) → TypeScript
   ↓
9. Upload Type Files + Metadata to S3/DynamoDB
   ↓
10. Download Type Files from Other Repos
    ↓
11. Run Cross-Repo Type Checking (ts_check) → TypeScript
    ↓
12. Merge All Results → Final Analysis Report
```

---

### 6.2 Type Checking Data Flow

```
Rust: Analyzer
  ├─ Extract type references from API calls
  │  └─ TypeInfo { filePath, startPosition, compositeTypeString, alias }
  ↓
TypeScript: extract-type-definitions.ts
  ├─ Load project with ts-morph
  ├─ Find type at position
  ├─ Recursively collect dependencies
  ├─ Generate standalone type file
  │  └─ repo-a_types.ts
  ↓
Rust: Upload to S3
  └─ S3: org/repo-a/commit-hash/types.ts
  └─ DynamoDB: metadata + S3 URL
  ↓
Rust: Download from S3 (cross-repo)
  └─ ts_check/output/repo-a_types.ts
  └─ ts_check/output/repo-b_types.ts
  ↓
TypeScript: run-type-checking.ts
  ├─ Load all type files
  ├─ Group by endpoint (producers vs consumers)
  ├─ Compare types with TypeScript compiler
  │  └─ producerType.isAssignableTo(consumerType)
  ├─ Get TypeScript diagnostics for errors
  │  └─ type-check-results.json
  ↓
Rust: Read Results
  └─ Include type mismatches in final report
```

---

## 7. Agent Responsibilities

### 7.1 TriageAgent

**Purpose**: Fast, broad categorization of all call sites

**LLM Usage**: 1 batch per 10 call sites, minimal data (lean)

---

### 7.2 EndpointAgent

**Purpose**: Extract method, path, handler from HTTP endpoint definitions

**LLM Usage**: 1 per batch, moderate data (full call sites)

---

### 7.3 ConsumerAgent

**Purpose**: Extract library, URL, method from API calls

**LLM Usage**: 1 per batch, moderate data (full call sites)

---

### 7.4 MountAgent

**Purpose**: Extract parent, child, path from router mounts

**LLM Usage**: 1 per batch, moderate data (full call sites)

---

### 7.5 MiddlewareAgent

**Purpose**: Classify and extract middleware registrations

**LLM Usage**: 1 per batch, moderate data (full call sites)

---

## 8. Technical Implementation Details

### 8.1 Batching Strategy

**Problem**: Large codebases with 100+ call sites cause LLM timeouts.

**Solution**: Process in batches of 10 with 500ms delay between batches.

---

### 8.2 Structured Output with Schemas

**Problem**: LLM responses can be unpredictable.

**Solution**: Gemini's structured output schemas enforce JSON format.

---

### 8.3 Path Resolution Algorithm

**Challenge**: Compute full endpoint paths when routers are mounted at prefixes.

**Implementation**:
```rust
pub fn compute_full_path(&self, owner: &str, path: &str) -> String {
    let mut full_path = path.to_string();
    let mut current_node = owner;
    
    // Walk up the mount chain
    while let Some(mount) = self.find_mount_for_child(current_node) {
        full_path = self.join_paths(&mount.path_prefix, &full_path);
        current_node = &mount.parent;
    }
    
    self.normalize_path(&full_path)
}
```

---

### 8.4 Type Extraction Workflow

**Step 1**: Gemini extracts API calls with type annotations
```rust
Call {
    route: "/api/users",
    method: "GET",
    response_type: Some(TypeReference {
        file_path: "src/api.ts",
        start_position: 1234,
        composite_type_string: "Response<User[]>",
        alias: "GetApiUsersResponseProducerRepoA"
    })
}
```

**Step 2**: Rust calls TypeScript extraction
```rust
extract_types_for_repo(repo_path, type_infos, packages)
```

**Step 3**: TypeScript generates standalone file
```typescript
// ts_check/output/repo-a_types.ts
export type GetApiUsersResponseProducerRepoA = Response<User[]>;
export interface User {
  id: number;
  name: string;
  email: string;
}
export interface Response<T> {
  data: T;
  status: number;
}
```

**Step 4**: Upload to S3 and store URL in DynamoDB

**Step 5**: In cross-repo analysis, download all type files

**Step 6**: Run TypeScript compiler
```typescript
// Checks if repo-a's producer type matches repo-b's consumer type
const producerType = repoA.GetApiUsersResponseProducerRepoA;
const consumerType = repoB.GetApiUsersResponseConsumerRepoB;
const isCompatible = producerType.isAssignableTo(consumerType);
```

---

## 9. Cross-Repository Analysis

### 9.1 Data Sharing Architecture

**Storage**:
- **DynamoDB**: Metadata (endpoints, calls, mounts, S3 URLs)
- **S3**: Type files (large TypeScript definitions)

**Organization**:
- All repos in same org share data
- Keyed by: `org/repo-name/commit-hash`

---

### 9.2 Upload Flow (On main/master branch)

```
1. Analyze current repo
   ↓
2. Extract types → ts_check/output/repo_types.ts
   ↓
3. Upload type file to S3
   └─ Returns S3 URL
   ↓
4. Upload metadata to DynamoDB
   └─ Includes: endpoints, calls, mounts, S3 URL, commit hash
```

---

### 9.3 Download Flow (On all branches, including PRs)

```
1. Query DynamoDB for org's repos
   └─ Returns: metadata + S3 URLs
   ↓
2. Download all type files from S3
   └─ Writes to ts_check/output/
   ↓
3. Create package.json with all dependencies
   ↓
4. Run npm install
   ↓
5. Run type checking
   └─ Compares all producer/consumer types
   ↓
6. Report results
```

---

### 9.4 Type File Naming

**Format**: `{org}_{repo-name}_types.ts`

**Example**:
- `myorg_user-service_types.ts`
- `myorg_order-service_types.ts`
- `myorg_comment-service_types.ts`

**Slashes replaced with underscores** for filesystem compatibility.

---

## 12. Legacy Code to Remove

### ⚠️ CRITICAL DIRECTIVE: DO NOT MAINTAIN LEGACY CODE

The following code exists for backward compatibility during migration. **It must be removed**, not refactored or improved.

### 12.1 Files to Delete/Refactor

**High Priority for Deletion**:
1. `src/visitor.rs` - Express-specific AST visitors
   - `DependencyVisitor`: Uses framework patterns like `express.Router()`
   - Replace with: Multi-agent orchestrator results

2. `src/extractor.rs` - Framework-specific extraction logic
   - Pattern matching for Express routes
   - Replace with: Agent-based extraction

3. Parts of `src/analyzer/mod.rs`:
   - Methods using old visitor patterns
   - Express-specific route resolution
   - Keep: Type checking orchestration, cross-repo analysis

**Adapter Code to Remove**:
- `convert_orchestrator_results_to_analyzer_data()` in `src/engine/mod.rs`
- Any code converting between `ApiEndpointDetails` and agent results

### 12.2 Pattern Matching to Eliminate

**Search for and Delete**:
```rust
// WRONG: Framework-specific checks
if definition.contains("express.Router()") { ... }
if definition.contains("Router()") { ... }
if callee_property == "get" && is_express_app { ... }
```

**Replace With**:
```rust
// RIGHT: Behavior-based classification
if mounted_nodes.contains(&node_name) {
    node_type = NodeType::Mountable;
}
```

### 12.3 Why Legacy Code Must Go

**Problems with Legacy Approach**:
1. **Brittle**: Breaks with new frameworks or patterns
2. **Incomplete**: Misses complex routing scenarios
3. **Maintenance Burden**: Every new framework needs new patterns
4. **Conflicts**: Legacy and new code produce different results

**Multi-Agent Advantages**:
1. **Universal**: Works with any framework via LLM understanding
2. **Adaptable**: No code changes for new frameworks
3. **Accurate**: LLM handles edge cases better than regex
4. **Maintainable**: Single code path, clear responsibilities

### 12.4 Migration Checklist

- [ ] Extract types from multi-agent results (not just Gemini)
- [ ] Remove `src/visitor.rs` Express-specific code
- [ ] Remove framework pattern matching from `src/analyzer/`
- [ ] Delete adapter functions in `src/engine/mod.rs`
- [ ] Build `CloudRepoData` directly from multi-agent results
- [ ] Add integration tests with multiple frameworks
- [ ] Validate output matches across Express/Fastify/Koa
- [ ] Remove `src/extractor.rs` if no longer needed
- [ ] Update documentation to reflect single analysis path

---

## 11. LLM Optimization Strategies

### 11.1 Two-Stage Classification

**Triage**: 30-50% smaller prompts (lean call sites)
**Specialist**: Only process relevant call sites

---

### 11.2 Batching and Retry Logic

**Batch Size**: 10 call sites per request
**Delay**: 500ms between batches
**Retry**: 3 attempts with exponential backoff for 503 errors

---

### 11.3 Parallel Agent Execution

**Implementation**: `tokio::try_join!` runs all specialist agents concurrently

**Benefit**: ~4x speedup when all agents have work

---

### 11.4 Structured Output Schemas

**Benefit**: Guaranteed parseable JSON, no manual cleanup

---

### 11.5 Framework Context Injection

**Implementation**: Include framework detection in every prompt

**Benefit**: Context-aware decisions without hardcoding

---

## 12. Future Considerations

### 12.1 Type Extraction Improvements

**Current Gap**: Types only extracted from Gemini calls (not multi-agent results yet)

**Future Work**:
- Extract types from EndpointAgent results
- Extract types from ConsumerAgent results
- Integrate type extraction into agent workflow

---

### 12.2 Enhanced Type Checking

**Potential Improvements**:
- Runtime type validation generation
- GraphQL schema compatibility
- OpenAPI spec generation from types
- Type-driven test generation

---

### 12.3 Performance Optimization

**Current Bottleneck**: LLM API calls + npm install

**Future Optimizations**:
- Caching: Cache triage results for unchanged files
- Incremental Analysis: Only analyze changed files
- Dependency Caching: Reuse node_modules between runs
- Parallel Type Extraction: Extract types per repo concurrently

---

### 12.4 Additional Specialist Agents

**Potential Additions**:
- **TypeExtractionAgent**: Dedicated type extraction from code
- **GraphQLAgent**: Detect GraphQL schemas and queries
- **AuthAgent**: Extract authentication/authorization patterns
- **ErrorHandlingAgent**: Identify error handling patterns

---

## Appendix A: Key Files Reference

| File | Purpose | Role |
|------|---------|------|
| `multi_agent_orchestrator.rs` | Top-level coordinator | Runs complete workflow |
| `call_site_extractor.rs` | Universal extraction | Extracts all call sites |
| `framework_detector.rs` | Framework identification | Detects frameworks/libraries |
| `agents/orchestrator.rs` | Classify-then-dispatch | Triage → dispatch pattern |
| `agents/triage_agent.rs` | Broad classification | Lightweight categorization |
| `agents/endpoint_agent.rs` | Endpoint details | Extracts HTTP endpoints |
| `agents/consumer_agent.rs` | Consumer details | Extracts API calls |
| `agents/mount_agent.rs` | Mount relationships | Extracts router mounts |
| `mount_graph.rs` | Graph construction | Builds mount graph |
| `analyzer/mod.rs` | Analysis coordinator | Type extraction + checking |
| `engine/mod.rs` | Engine coordinator | Cross-repo orchestration |
| `ts_check/extract-type-definitions.ts` | Type extraction | Extracts TypeScript types |
| `ts_check/run-type-checking.ts` | Type checking | Validates type compatibility |
| `ts_check/lib/type-extractor.ts` | Type extraction logic | Orchestrates extraction |
| `ts_check/lib/type-checker.ts` | Type comparison | TypeScript type validation |

---

## Appendix B: Type Checking Flow Diagram

```
┌─────────────────────────────────────────────────────────────┐
│ RUST: API Call Extraction (Gemini + Multi-Agent)           │
│ - Extract endpoints: GET /api/users                         │
│ - Extract calls: axios.get('/api/users')                    │
│ - Extract types: Response<User[]>, User                     │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ RUST → TypeScript: extract-type-definitions.ts             │
│ Input: [{ filePath, startPosition, compositeTypeString }]  │
│ Process:                                                     │
│   1. Load project with ts-morph                             │
│   2. Find type at position                                  │
│   3. Recursively collect all dependencies                   │
│   4. Generate standalone file                               │
│ Output: ts_check/output/repo-a_types.ts                    │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ RUST: Upload to Cloud Storage                               │
│ - S3: org/repo-a/commit-hash/types.ts                      │
│ - DynamoDB: metadata + S3 URL                              │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓ (Cross-Repo Analysis)
┌─────────────────────────────────────────────────────────────┐
│ RUST: Download Type Files from All Repos                    │
│ - Query DynamoDB for org                                    │
│ - Download all type files from S3                          │
│ - Write to ts_check/output/                                │
│   ├─ repo-a_types.ts                                       │
│   ├─ repo-b_types.ts                                       │
│   └─ repo-c_types.ts                                       │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ RUST → TypeScript: run-type-checking.ts                    │
│ Process:                                                     │
│   1. npm install (install all dependencies)                │
│   2. Load all type files with ts-morph                     │
│   3. Parse type names → extract endpoints                  │
│   4. Group by endpoint (producers vs consumers)            │
│   5. Compare types using TypeScript compiler               │
│      - producerType.isAssignableTo(consumerType)          │
│   6. Get TypeScript diagnostic messages                    │
│ Output: ts_check/output/type-check-results.json           │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ RUST: Read Type Check Results                               │
│ - Parse type-check-results.json                            │
│ - Add type mismatches to ApiIssues                         │
│ - Include in final analysis report                         │
└─────────────────────────────────────────────────────────────┘
```

---

## Appendix C: Example Type Mismatch Detection

### Producer (repo-a):

```typescript
// src/api.ts
app.get('/users/:id', (req, res) => {
  const user: User = { id: 1, name: "Alice" };
  res.json(user);
});

interface User {
  id: number;
  name: string;
}
```

**Generated Type File** (`repo-a_types.ts`):
```typescript
export type GetUsersByIdResponseProducerRepoA = User;
export interface User {
  id: number;
  name: string;
}
```

### Consumer (repo-b):

```typescript
// src/client.ts
interface User {
  id: number;
  name: string;
  email: string;  // Missing in producer!
}

async function getUser(id: string): Promise<User> {
  const response = await fetch(`${API_URL}/users/${id}`);
  return response.json();
}
```

**Generated Type File** (`repo-b_types.ts`):
```typescript
export type GetUsersByIdResponseConsumerCall1RepoB = User;
export interface User {
  id: number;
  name: string;
  email: string;
}
```

### Type Checking Result:

```typescript
// ts_check/run-type-checking.ts compares:
const producerType = repoA.GetUsersByIdResponseProducerRepoA; // { id, name }
const consumerType = repoB.GetUsersByIdResponseConsumerCall1RepoB; // { id, name, email }

const isCompatible = producerType.isAssignableTo(consumerType);
// Returns: false

const error = getTypeCompatibilityError(...);
// Returns: "Property 'email' is missing in type '{ id: number; name: string; }' 
//           but required in type '{ id: number; name: string; email: string; }'."
```

**Output** (`type-check-results.json`):
```json
{
  "mismatches": [
    {
      "endpoint": "GET /users/:id",
      "producerType": "{ id: number; name: string; }",
      "consumerType": "{ id: number; name: string; email: string; }",
      "error": "Property 'email' is missing in producer type",
      "isCompatible": false
    }
  ]
}
```

---

## Conclusion

Carrick's multi-agent architecture represents a **complete paradigm shift** in API analysis:

**1. Framework-Agnostic Analysis**
- LLM-powered semantic understanding
- Works with any JavaScript/TypeScript framework
- No hardcoded patterns

**2. Type Safety Across Repositories**
- TypeScript compiler validates type compatibility
- Catches type mismatches before deployment
- Accurate error messages from TypeScript

**3. Optimized LLM Usage**
- Classify-Then-Dispatch reduces costs
- Batching prevents timeouts
- Parallel execution improves speed

**4. Complete Cross-Repo Analysis**
- Shared type files via S3
- Metadata coordination via DynamoDB
- Detects API mismatches across services

**Key Achievement**: A truly framework-agnostic tool that combines the power of LLMs for semantic understanding with TypeScript's type system for rigorous validation—all while maintaining backward compatibility with existing analysis infrastructure.

---

**Document Version**: 2.0 (Complete)  
**Last Updated**: 2025-11-10  
**Author**: Comprehensive analysis of Carrick codebase including ts_check/
