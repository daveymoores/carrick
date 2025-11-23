# Carrick Multi-Agent Framework-Agnostic Architecture: Comprehensive Analysis

**Document Version**: 1.0
**Date**: November 16, 2025
**Branch**: multi-agent (created from main)
**Source Analysis**: multi-agent-workflow branch research

---

## Executive Summary

This document provides a detailed analysis of Carrick's proposed transformation from a framework-specific (Express-focused) API analysis tool into a **fully framework-agnostic multi-agent system**. The analysis synthesizes insights from the multi-agent-workflow branch research document with the existing project architecture documented in `.thoughts/research/`.

### Key Findings

1. **Vision is Sound**: The multi-agent architecture represents a sophisticated solution for framework-agnostic analysis
2. **Critical Blocker Exists**: The implementation has a zero-output bug that prevents any analysis results
3. **Dual Analysis Paths**: The system currently maintains both legacy (working) and multi-agent (broken) paths
4. **TypeScript Integration**: The `ts_check/` system is already framework-agnostic and well-designed
5. **Complete Removal Required**: Legacy framework-specific code must be eliminated, not maintained

### What This Analysis Covers

- **Architecture Comparison**: Legacy vs Multi-Agent approaches
- **Implementation Status**: What works, what's broken, what's missing
- **Technical Deep Dive**: How the multi-agent system should work
- **Root Cause Analysis**: Why the current implementation produces zero output
- **Path Forward**: Concrete steps to complete the transformation
- **Framework Agnosticism**: True framework independence without hardcoded patterns

---

## Table of Contents

1. [Project Context](#1-project-context)
2. [The Multi-Agent Vision](#2-the-multi-agent-vision)
3. [Current Architecture State](#3-current-architecture-state)
4. [Critical Bug Analysis: Zero Output](#4-critical-bug-analysis-zero-output)
5. [Framework Agnosticism Deep Dive](#5-framework-agnosticism-deep-dive)
6. [Multi-Agent System Design](#6-multi-agent-system-design)
7. [TypeScript Type Checking Integration](#7-typescript-type-checking-integration)
8. [Implementation Gaps](#8-implementation-gaps)
9. [Legacy Code Removal Strategy](#9-legacy-code-removal-strategy)
10. [Path to Completion](#10-path-to-completion)
11. [Success Criteria](#11-success-criteria)
12. [Risk Assessment](#12-risk-assessment)

---

## 1. Project Context

### 1.1 What is Carrick?

Carrick is a **cross-repository API consistency analysis tool** written in Rust that detects:
- API endpoint mismatches
- TypeScript type incompatibilities
- Dependency version conflicts
- Configuration issues

across microservices architectures before they reach production.

### 1.2 Current State (main branch)

**Technology Stack:**
- **Core**: Rust with SWC (Speedy Web Compiler) for AST parsing
- **AI**: Google Gemini 2.5 Flash for complex code analysis
- **Type Checking**: TypeScript Compiler API via ts-morph
- **Cloud**: AWS (Lambda, DynamoDB, S3, API Gateway)
- **IaC**: Terraform

**Key Components:**
1. Parser layer (SWC-based)
2. Visitor layer (Express-specific AST traversal)
3. AI extraction layer (Gemini for dynamic URL extraction)
4. Analyzer layer (matches endpoints with calls)
5. Cloud storage layer (AWS for cross-repo data)
6. Type checking system (`ts_check/` - TypeScript-based)
7. Output formatter (GitHub markdown)

**Limitations:**
- **Framework-Specific**: Hardcoded Express patterns (`express.Router()`, `app.get()`, etc.)
- **Brittle Pattern Matching**: Breaks with new frameworks or non-standard patterns
- **Maintenance Burden**: Each new framework requires new pattern recognition code

### 1.3 The Problem Being Solved

**Current Approach Issues:**

```rust
// Example of brittle framework-specific code
if definition.contains("express.Router()") {
    node_type = NodeType::Router;
}

if callee_property == "get" && is_express_app {
    extract_endpoint(...);
}
```

**Problems:**
1. Only works with Express
2. Misses complex routing patterns
3. Cannot handle Fastify, Koa, Hapi, NestJS, or custom frameworks
4. Every framework variation requires code changes
5. Maintenance cost scales linearly with framework support

### 1.4 Testing Infrastructure

The project has **excellent test coverage** (43 tests, all passing):
- 12 unit tests (pure functions)
- 4 output contract tests (fixture-based)
- 10 endpoint matching tests
- 10 MockStorage tests
- 4 dependency analysis tests
- 3 integration tests (full binary)

**Key Insight**: The **output-focused testing strategy** is perfect for validating a multi-agent refactor because it doesn't test implementation details.

---

## 2. The Multi-Agent Vision

### 2.1 Core Philosophy

**From**: Pattern-matching specific frameworks
**To**: Understanding code behavior through LLM-powered semantic analysis

**Key Principles:**
1. **Behavioral Classification**: Classify code by what it *does*, not by matching patterns
2. **Universal Extraction**: Extract all call sites, let LLM decide what matters
3. **Semantic Understanding**: Use LLM context to understand code semantics
4. **Type Safety**: Use TypeScript compiler for validation (already framework-agnostic)

### 2.2 The Multi-Agent Approach

#### Classify-Then-Dispatch Pattern

```
Stage 0: Framework Detection (LLM-powered)
    ↓
Stage 1: Universal Call Site Extraction (AST-based, framework-agnostic)
    ↓
Stage 2: Triage Classification (LLM categorizes all call sites)
    ↓
Stage 3: Specialist Agent Dispatch (Parallel LLM calls for details)
    ├─ EndpointAgent (extracts HTTP endpoint definitions)
    ├─ ConsumerAgent (extracts outbound API calls)
    ├─ MountAgent (extracts router mounting)
    └─ MiddlewareAgent (extracts middleware registration)
    ↓
Stage 4: Mount Graph Construction (behavior-based path resolution)
    ↓
Stage 5: Type Extraction & Checking (TypeScript compiler)
    ↓
Result: Framework-Agnostic API Analysis with Type Safety
```

### 2.3 Why This Approach Works

**Universal Call Site Extraction:**
```rust
// Extract ALL member call expressions
// object.property(args)
// Examples: app.get(), router.use(), axios.get(), fetch()
pub struct CallSite {
    pub callee_object: String,   // "app", "router", "axios"
    pub callee_property: String,  // "get", "post", "use"
    pub args: Vec<CallArgument>,
    pub definition: Option<String>,
    pub location: String,
}
```

**LLM-Powered Classification:**
- No hardcoded patterns
- Context-aware (knows about detected frameworks)
- Handles edge cases naturally
- Works with custom frameworks

**Behavior-Based Path Resolution:**
```rust
// Don't check if it's "express.Router()"
// Instead: classify by mount behavior
if mounted_nodes.contains(&node_name) {
    node_type = NodeType::Mountable;
}
```

### 2.4 Expected Benefits

1. **True Framework Agnosticism**: Works with Express, Fastify, Koa, Hapi, NestJS, or custom frameworks
2. **Reduced Maintenance**: Single analysis path, no per-framework code
3. **Better Accuracy**: LLM handles complex patterns better than regex
4. **Future-Proof**: New frameworks work automatically
5. **Cleaner Codebase**: No legacy pattern matching clutter

---

## 3. Current Architecture State

### 3.1 Dual Implementation Reality

**The project currently has TWO analysis paths:**

#### Path 1: Legacy (main branch) - WORKS
```
Files → Parser → DependencyVisitor → Express-specific patterns
  → Analyzer → Gemini (async functions) → Type Checking
  → CloudStorage → Cross-Repo Analysis → Report
```

**Status**: ✅ Functional, produces output, 43 tests pass

#### Path 2: Multi-Agent (multi-agent-workflow branch) - BROKEN
```
Files → Parser → CallSiteExtractor → Framework Detection
  → TriageAgent → Specialist Agents → MountGraph
  → Analyzer → Type Checking → CloudStorage → Report
```

**Status**: ❌ Broken, produces **0 endpoints, 0 calls**

### 3.2 Implementation Status Matrix

| Component | Status | Notes |
|-----------|--------|-------|
| **CallSiteExtractor** | ✅ Complete | Framework-agnostic AST visitor |
| **FrameworkDetector** | ✅ Complete | LLM-powered detection |
| **TriageAgent** | ✅ Complete | Categorizes call sites |
| **EndpointAgent** | ✅ Complete | Extracts endpoint details |
| **ConsumerAgent** | ✅ Complete | Extracts API call details |
| **MountAgent** | ✅ Complete | Extracts mount relationships |
| **MiddlewareAgent** | ✅ Complete | Extracts middleware |
| **CallSiteOrchestrator** | ✅ Complete | Classify-Then-Dispatch logic |
| **MountGraph** | ✅ Complete | Behavior-based path resolution |
| **MultiAgentOrchestrator** | ✅ Complete | Top-level coordinator |
| **Agent-Analyzer Integration** | ❌ BROKEN | Produces zero output |
| **Type Extraction from Agents** | ❌ Missing | Only works with Gemini path |
| **Direct CloudRepoData Build** | ❌ Missing | Uses adapter pattern |
| **Legacy Code Removal** | ❌ Not Started | Old code still exists |

### 3.3 File Structure Overview

**Multi-Agent System Files:**
```
src/
├── multi_agent_orchestrator.rs      # Top-level coordinator
├── call_site_extractor.rs           # Universal call site extraction
├── framework_detector.rs            # LLM framework detection
├── agents/
│   ├── orchestrator.rs              # Classify-Then-Dispatch
│   ├── triage_agent.rs             # Categorization
│   ├── endpoint_agent.rs           # Endpoint extraction
│   ├── consumer_agent.rs           # API call extraction
│   ├── mount_agent.rs              # Mount extraction
│   └── middleware_agent.rs         # Middleware extraction
├── mount_graph.rs                   # Graph construction
└── ...
```

**Legacy System Files (to be removed):**
```
src/
├── visitor.rs                       # Express-specific visitors
├── extractor.rs                     # Framework-specific extraction
├── analyzer/mod.rs                  # Contains legacy patterns
└── ...
```

**Framework-Agnostic Components (keep):**
```
src/
├── parser.rs                        # SWC parser (universal)
├── gemini_service.rs               # LLM service (universal)
├── cloud_storage/                   # AWS integration (universal)
├── formatter/                       # Output formatting (universal)
└── config.rs                        # Configuration (universal)

ts_check/                            # TypeScript type checking (universal)
├── extract-type-definitions.ts
├── run-type-checking.ts
└── lib/
```

---

## 4. Critical Bug Analysis: Zero Output

### 4.1 The Symptom

When running the multi-agent system on `tests/fixtures/imported-routers`:

```
Found 4 files to analyze in directory tests/fixtures/imported-routers
Extracted 0 imported symbols from 4 files
Converted orchestrator results:
  - 0 endpoints
  - 0 calls
  - 0 mounts

Analyzed **0 endpoints** and **0 API calls** across all repositories.
```

**Expected Output:**
- 3-4 endpoints (`/users`, `/api/v1`, `/health`)
- 3 mount relationships
- Multiple call sites extracted

### 4.2 Root Cause Hypotheses

Based on the multi_agent_architecture.md document, there are four potential failure points:

#### Hypothesis 1: CallSiteExtractor Not Extracting
**Symptom**: `call_sites.len() == 0` after Stage 1

**Possible Causes:**
- SWC parser errors (fails silently on some TypeScript features)
- AST visitor not traversing member expressions correctly
- File discovery issues (files not being parsed)

**Test:**
```rust
// In src/call_site_extractor.rs
println!("DEBUG: Extracted {} call sites from file: {:?}",
         call_sites.len(), file_path);
```

#### Hypothesis 2: Agents Not Being Called
**Symptom**: `call_sites.len() > 0` but `analysis_results` is empty

**Possible Causes:**
- Gemini API not being invoked
- API key issues (using "mock" instead of real key)
- Network connectivity problems
- Agent batch processing errors

**Test:**
```rust
// In src/agents/orchestrator.rs
println!("DEBUG: Triage classifying {} call sites", call_sites.len());
println!("DEBUG: Triage results: {} endpoints, {} calls",
         endpoints.len(), calls.len());
```

#### Hypothesis 3: MountGraph Not Receiving Data
**Symptom**: Agent results populated but `mount_graph` is empty

**Possible Causes:**
- `AnalysisResults` struct not properly populated
- `MountGraph::build_from_analysis_results()` not copying data
- Field mapping issues

**Test:**
```rust
// In src/mount_graph.rs
println!("DEBUG: Building graph from {} endpoints, {} calls",
         analysis_results.endpoints.len(),
         analysis_results.data_fetching_calls.len());
```

#### Hypothesis 4: Conversion Layer Issues
**Symptom**: Mount graph populated but converter produces empty vectors

**Possible Causes:**
- `convert_orchestrator_results_to_analyzer_data()` mapping errors
- Field name mismatches
- Empty slice access issues

**Test:**
```rust
// In src/engine/mod.rs
let resolved_endpoints = mount_graph.get_resolved_endpoints();
println!("DEBUG: MountGraph has {} resolved endpoints",
         resolved_endpoints.len());
```

### 4.3 Most Likely Root Cause

**Primary Suspect**: Hypothesis 2 (Agents Not Being Called)

**Evidence from research document:**
> "The multi-agent workflow is implemented but **agents are not actually calling Gemini to extract data**. The orchestrator runs through the motions but produces empty results."

**Why This Makes Sense:**
1. File discovery works (4 files found)
2. Framework detection likely works (can detect Express from package.json)
3. But if agents never call Gemini, they can't extract details
4. The system would run without errors but produce empty results

**What to Check:**
- Is `CARRICK_API_KEY` a real API key or just "mock"?
- Is Gemini API endpoint accessible?
- Are there network errors in logs?
- Is `gemini_service.analyze_code_with_schema()` being called?

### 4.4 Debugging Strategy

**Phase 1: Add Comprehensive Logging**

```rust
// src/multi_agent_orchestrator.rs:run_complete_analysis()
println!("=== STAGE 0: Framework Detection ===");
let framework_detection = framework_detector
    .detect_frameworks_and_libraries(packages, imported_symbols)
    .await?;
println!("Detected frameworks: {:?}", framework_detection.frameworks);

println!("=== STAGE 1: Call Site Extraction ===");
let call_sites = self.extract_all_call_sites(&files).await?;
println!("Extracted {} total call sites", call_sites.len());
println!("Sample: {:?}", call_sites.iter().take(3).collect::<Vec<_>>());

println!("=== STAGE 2: Classify-Then-Dispatch ===");
let orchestrator = CallSiteOrchestrator::new(self.gemini_service.clone());
let analysis_results = orchestrator
    .analyze_call_sites(&call_sites, &framework_detection)
    .await?;
println!("Analysis results: {} endpoints, {} calls, {} mounts",
         analysis_results.endpoints.len(),
         analysis_results.data_fetching_calls.len(),
         analysis_results.mount_relationships.len());

println!("=== STAGE 3: Mount Graph ===");
let mount_graph = MountGraph::build_from_analysis_results(&analysis_results);
println!("Mount graph: {} endpoints, {} data calls",
         mount_graph.get_resolved_endpoints().len(),
         mount_graph.get_data_calls().len());
```

**Phase 2: Run with Real API Key**

```bash
# Get real API key from Google AI Studio
export CARRICK_API_KEY="actual-api-key"
export CARRICK_API_ENDPOINT="http://localhost:3000"  # or Lambda endpoint
export CARRICK_MOCK_ALL=1
export CARRICK_ORG=test-org

cargo run -- tests/fixtures/imported-routers 2>&1 | tee debug.log
grep "DEBUG\|===" debug.log
```

**Phase 3: Isolate Failure Point**

Based on where the count drops to zero, fix that specific component.

---

## 5. Framework Agnosticism Deep Dive

### 5.1 What Framework Agnosticism Really Means

**Not Framework-Agnostic (Legacy):**
```rust
// Hardcoded Express patterns
if definition.contains("express.Router()") {
    is_router = true;
}

if definition.contains("express()") && callee_property == "use" {
    extract_mount(...);
}

// Breaks with Fastify:
const app = fastify();
app.register(routes, { prefix: '/api' });  // Not detected!
```

**Truly Framework-Agnostic (Multi-Agent):**
```rust
// Universal call site extraction
CallSite {
    callee_object: "app",
    callee_property: "register",
    args: [...]
}

// LLM determines meaning based on framework context
TriageAgent::classify(call_site, framework="fastify")
  → Result: RouterMount
```

### 5.2 Framework Detection Strategy

**Input Sources:**
1. `package.json` dependencies
2. Import statements in source code

**LLM Prompt Strategy:**
```
Given these dependencies and imports, identify:
1. HTTP frameworks (Express, Fastify, Koa, Hapi, NestJS, etc.)
2. Data-fetching libraries (axios, fetch, got, graphql-request, etc.)

Output JSON:
{
  "frameworks": ["express"],
  "data_fetchers": ["axios", "fetch"]
}
```

**Why This Works:**
- LLM has knowledge of all major frameworks
- Can identify custom frameworks by usage patterns
- Doesn't require maintaining a framework database

### 5.3 Call Site Extraction: Universal Pattern

**What Gets Extracted:**

```typescript
// Express
app.get('/users', handler);              → CallSite
app.use('/api', router);                 → CallSite

// Fastify
app.register(routes, { prefix: '/api' });→ CallSite
app.get('/users', handler);              → CallSite

// Koa
router.get('/users', handler);           → CallSite
app.use(router.routes());                → CallSite

// Custom Framework
framework.addRoute('GET', '/users');     → CallSite
framework.mount('/api', module);         → CallSite

// API Calls (any library)
axios.get('/api/users');                 → CallSite
fetch('/api/users');                     → CallSite (special case: direct call)
```

**Key Insight**: The pattern `object.property(args)` is universal across all frameworks.

### 5.4 Triage Classification: Context-Aware

**Agent Input:**
```rust
LeanCallSite {
    object: "app",
    property: "get",
    first_arg: Some("/users"),
    definition: Some("const app = express()"),
}

FrameworkContext {
    frameworks: ["express"],
    data_fetchers: ["axios"],
}
```

**LLM Decision Process:**
1. See that `app` is an Express app (from definition)
2. Know that `app.get()` in Express defines HTTP endpoints
3. Classify as `HttpEndpoint`

**Same Pattern, Different Framework:**
```rust
LeanCallSite {
    object: "axios",
    property: "get",
    first_arg: Some("/api/users"),
}

FrameworkContext {
    frameworks: ["express"],
    data_fetchers: ["axios"],
}
```

**LLM Decision:**
1. See that `axios` is a data fetcher
2. Know that `axios.get()` makes HTTP requests
3. Classify as `DataFetchingCall`

### 5.5 Mount Graph: Behavior-Based Classification

**Old Way (Pattern Matching):**
```rust
// BRITTLE: Assumes Express-specific patterns
if definition.contains("express.Router()") {
    node_type = NodeType::Router;
} else if definition.contains("express()") {
    node_type = NodeType::App;
}
```

**New Way (Behavior-Based):**
```rust
// UNIVERSAL: Classify by mount relationships
let mounted_nodes = mount_relationships
    .iter()
    .map(|m| &m.child)
    .collect::<HashSet<_>>();

let parent_nodes = mount_relationships
    .iter()
    .map(|m| &m.parent)
    .collect::<HashSet<_>>();

for node_name in all_nodes {
    if mounted_nodes.contains(&node_name) {
        // This node gets mounted → Mountable (router-like)
        node_type = NodeType::Mountable;
    } else if parent_nodes.contains(&node_name) {
        // This node mounts others → Root (app-like)
        node_type = NodeType::Root;
    }
}
```

**Why This Works:**
- No framework assumptions
- Works with Express, Fastify, Koa, custom frameworks
- Focuses on behavior, not syntax

### 5.6 Type Checking: Already Framework-Agnostic

**The `ts_check/` system is beautifully framework-agnostic:**

1. **Type Extraction**: Uses `ts-morph` to parse TypeScript AST
   - Works with any TypeScript code
   - Framework-independent

2. **Type Comparison**: Uses TypeScript compiler
   - `producerType.isAssignableTo(consumerType)`
   - Framework-independent

3. **Error Messages**: TypeScript diagnostics
   - Clear, accurate error messages
   - Framework-independent

**This component requires ZERO changes for framework agnosticism.**

---

## 6. Multi-Agent System Design

### 6.1 Stage-by-Stage Breakdown

#### Stage 0: Framework Detection

**File**: `src/framework_detector.rs`

**Purpose**: Identify frameworks and libraries used

**Input:**
```rust
DetectionInput {
    packages: Packages,              // From package.json
    imported_symbols: HashMap<...>,  // From import statements
}
```

**Process:**
1. Extract package names and versions
2. Extract import statement patterns
3. Send to LLM with classification prompt
4. Parse JSON response

**Output:**
```rust
DetectionResult {
    frameworks: ["express", "passport"],
    data_fetchers: ["axios", "node-fetch"],
    notes: "Express-based REST API with Passport authentication"
}
```

**LLM Call**: 1 per repository (cheap, one-time)

#### Stage 1: Universal Call Site Extraction

**File**: `src/call_site_extractor.rs`

**Purpose**: Extract ALL `object.property(args)` patterns

**Process:**
1. Use SWC to parse file into AST
2. Visit all `CallExpression` nodes
3. For each node with `MemberExpression` callee:
   - Extract object name
   - Extract property name
   - Extract arguments
   - Extract variable definition context
   - Record location

**Output:**
```rust
Vec<CallSite> {
    CallSite {
        callee_object: "app",
        callee_property: "get",
        args: [StringLiteral("/users"), Function(...)],
        definition: Some("const app = express()"),
        location: "app.ts:10:0",
    },
    // ... hundreds more
}
```

**No LLM Calls**: Pure AST traversal

**Key Optimization**: Extract everything, filter nothing. Let the LLM decide what matters.

#### Stage 2: Triage Classification

**File**: `src/agents/triage_agent.rs`

**Purpose**: Fast categorization of all call sites

**Optimization**: Uses `LeanCallSite` (30-50% smaller payload)

```rust
LeanCallSite {
    object: "app",
    property: "get",
    first_arg: Some("/users"),
    definition: Some("const app = express()"),
    location: "app.ts:10:0",
}
```

**Categories:**
```rust
enum TriageClassification {
    HttpEndpoint,        // Route definitions
    DataFetchingCall,    // API calls
    RouterMount,         // Router mounting
    Middleware,          // Middleware registration
    Irrelevant,          // Logging, utils, etc.
}
```

**LLM Calls**: 1 per batch of 10 call sites

**Gemini Structured Output Schema:**
```json
{
  "type": "array",
  "items": {
    "type": "object",
    "properties": {
      "index": {"type": "integer"},
      "classification": {
        "type": "string",
        "enum": ["HttpEndpoint", "DataFetchingCall", "RouterMount", "Middleware", "Irrelevant"]
      },
      "confidence": {"type": "string", "enum": ["High", "Medium", "Low"]}
    },
    "required": ["index", "classification"]
  }
}
```

**Result:**
```rust
TriageResult {
    index: 0,
    classification: HttpEndpoint,
    confidence: High,
}
```

#### Stage 3: Specialist Agent Dispatch

**File**: `src/agents/orchestrator.rs`

**Pattern**: Parallel execution with `tokio::try_join!`

**Agents:**

##### EndpointAgent
**Purpose**: Extract HTTP endpoint details

**Input**: Full `CallSite` (not lean)

**Output:**
```rust
HttpEndpoint {
    method: "GET",
    path: "/users",
    handler_name: Some("getUsers"),
    owner: "app",
    location: "app.ts:10:0",
}
```

**LLM Schema:**
```json
{
  "type": "object",
  "properties": {
    "method": {"type": "string"},
    "path": {"type": "string"},
    "handler_name": {"type": "string"},
    "owner": {"type": "string"}
  },
  "required": ["method", "path", "owner"]
}
```

##### ConsumerAgent
**Purpose**: Extract outbound API call details

**Output:**
```rust
DataFetchingCall {
    library: "axios",
    target_url: "/api/users",
    method: "GET",
    location: "client.ts:42:5",
}
```

##### MountAgent
**Purpose**: Extract router mount relationships

**Output:**
```rust
MountRelationship {
    parent: "app",
    child: "apiRouter",
    path_prefix: "/api",
    location: "app.ts:5:0",
}
```

##### MiddlewareAgent
**Purpose**: Extract middleware registrations

**Output:**
```rust
MiddlewareRegistration {
    owner: "app",
    middleware_name: "express.json",
    path: None,  // Global middleware
}
```

**Batching**: Process 10 call sites per agent per request

**LLM Calls**: ~4 parallel calls per batch (one per agent type)

#### Stage 4: Mount Graph Construction

**File**: `src/mount_graph.rs`

**Purpose**: Build graph of routing hierarchy

**Input:**
```rust
AnalysisResults {
    endpoints: Vec<HttpEndpoint>,
    data_fetching_calls: Vec<DataFetchingCall>,
    mount_relationships: Vec<MountRelationship>,
    middleware: Vec<MiddlewareRegistration>,
}
```

**Process:**

1. **Collect Nodes**: Identify all apps and routers
2. **Build Mounts**: Create parent-child relationships
3. **Classify Nodes**: Use behavior-based classification
4. **Add Endpoints**: Associate endpoints with owners
5. **Resolve Paths**: Walk mount chain to compute full paths

**Path Resolution Example:**
```rust
// Given:
MountRelationship { parent: "app", child: "apiRouter", path_prefix: "/api" }
MountRelationship { parent: "apiRouter", child: "v1Router", path_prefix: "/v1" }
HttpEndpoint { owner: "v1Router", path: "/users", method: "GET" }

// Compute full path:
walk_mount_chain("v1Router") → ["apiRouter", "app"]
combine_paths(["/api", "/v1", "/users"]) → "/api/v1/users"

// Result:
ResolvedEndpoint {
    method: "GET",
    full_path: "/api/v1/users",
    owner: "v1Router",
}
```

**No LLM Calls**: Pure graph traversal

**Output:**
```rust
MountGraph {
    nodes: HashMap<String, NodeInfo>,
    mounts: Vec<MountEdge>,
    endpoints: Vec<ResolvedEndpoint>,
    data_calls: Vec<DataFetchingCall>,
}
```

#### Stage 5: Type Extraction & Checking

**See Section 7 for detailed analysis**

### 6.2 Data Flow Diagram

```
┌─────────────────────────────────────────────────────────────┐
│ INPUT: JavaScript/TypeScript Files                          │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ STAGE 0: Framework Detection (LLM)                          │
│ - Parse package.json                                        │
│ - Extract import statements                                 │
│ - LLM classifies frameworks and libraries                   │
│ Output: DetectionResult                                     │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ STAGE 1: Call Site Extraction (AST)                        │
│ - Parse files with SWC                                      │
│ - Visit all CallExpression nodes                            │
│ - Extract object.property(args) patterns                    │
│ Output: Vec<CallSite>                                       │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ STAGE 2: Triage Classification (LLM)                       │
│ - Batch call sites (10 per request)                        │
│ - Convert to LeanCallSite                                   │
│ - LLM categorizes each                                      │
│ Output: Vec<TriageResult>                                   │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ STAGE 3: Specialist Agent Dispatch (Parallel LLM)          │
│                                                             │
│ ┌─────────────┐  ┌──────────────┐  ┌────────────┐         │
│ │ Endpoint    │  │ Consumer     │  │ Mount      │         │
│ │ Agent       │  │ Agent        │  │ Agent      │         │
│ │ (HTTP defs) │  │ (API calls)  │  │ (Mounts)   │         │
│ └──────┬──────┘  └──────┬───────┘  └─────┬──────┘         │
│        │                │                 │                │
│        └────────────────┴─────────────────┘                │
│                         │                                  │
│ Output: AnalysisResults {                                  │
│   endpoints, data_fetching_calls, mount_relationships      │
│ }                                                           │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ STAGE 4: Mount Graph Construction (Graph Algorithm)        │
│ - Collect nodes from results                               │
│ - Build mount relationships                                 │
│ - Classify nodes behaviorally                               │
│ - Resolve full paths                                        │
│ Output: MountGraph                                          │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ STAGE 5: Type Extraction & Checking (TypeScript)           │
│ - Extract type references from endpoints/calls              │
│ - Run ts_check/extract-type-definitions.ts                 │
│ - Generate standalone type files                            │
│ - Upload to S3, store metadata in DynamoDB                  │
│ - Download type files from other repos                      │
│ - Run ts_check/run-type-checking.ts                        │
│ - Report type compatibility issues                          │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ OUTPUT: Final Analysis Report                               │
│ - Endpoints found                                           │
│ - API calls made                                            │
│ - Missing endpoints (connectivity issues)                   │
│ - Type mismatches (compatibility issues)                    │
│ - Dependency conflicts                                      │
└─────────────────────────────────────────────────────────────┘
```

### 6.3 LLM Optimization Strategies

#### Strategy 1: Two-Stage Classification
**Benefit**: Reduces payload size by 30-50% for triage

#### Strategy 2: Batching
- **Batch Size**: 10 call sites per request
- **Delay**: 500ms between batches
- **Prevents**: LLM timeouts and rate limit errors

#### Strategy 3: Parallel Execution
```rust
let (endpoints, calls, mounts, middleware) = tokio::try_join!(
    endpoint_agent.extract(...),
    consumer_agent.extract(...),
    mount_agent.extract(...),
    middleware_agent.extract(...)
)?;
```
**Speedup**: ~4x when all agents have work

#### Strategy 4: Structured Output
- **Benefit**: Guaranteed parseable JSON
- **No Cleanup**: No need to strip markdown or parse freeform text
- **Type Safety**: Gemini enforces schema

#### Strategy 5: Framework Context Injection
- **Every prompt includes**: Detected frameworks and libraries
- **Benefit**: Context-aware decisions without additional LLM calls

---

## 7. TypeScript Type Checking Integration

### 7.1 System Overview

The `ts_check/` directory contains a **TypeScript-based type extraction and validation system** that's already framework-agnostic and well-designed.

**Key Insight**: This component is **already perfect** for framework-agnostic analysis. It doesn't need changes.

### 7.2 Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ Rust: Extract Type References                               │
│ - From Gemini-analyzed async functions (current)            │
│ - From agent-extracted endpoints (TODO)                     │
│ - From agent-extracted calls (TODO)                         │
│                                                             │
│ Output: TypeInfo {                                          │
│   filePath: "src/api.ts",                                  │
│   startPosition: 1234,                                      │
│   compositeTypeString: "Response<User[]>",                 │
│   alias: "GetApiUsersResponseProducerRepoA"                │
│ }                                                           │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ TypeScript: extract-type-definitions.ts                     │
│ - Load project with ts-morph                                │
│ - Find type at specified position                           │
│ - Recursively collect all type dependencies                 │
│ - Handle utility types (Response<T>, Pick<T, K>, etc.)     │
│ - Extract npm dependencies                                  │
│ - Generate standalone type file                             │
│                                                             │
│ Output: repo-a_types.ts                                    │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ Rust: Upload to Cloud Storage                               │
│ - Upload type file to S3                                    │
│ - Store metadata in DynamoDB (includes S3 URL)             │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓ (Cross-Repo Analysis)
┌─────────────────────────────────────────────────────────────┐
│ Rust: Download All Repo Type Files                          │
│ - Query DynamoDB for organization                           │
│ - Download all type files from S3                           │
│ - Write to ts_check/output/                                │
│   ├─ repo-a_types.ts                                       │
│   ├─ repo-b_types.ts                                       │
│   └─ repo-c_types.ts                                       │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ TypeScript: run-type-checking.ts                            │
│ - npm install (install all dependencies)                    │
│ - Load all type files with ts-morph                         │
│ - Parse type names → extract endpoints                      │
│ - Group by endpoint (producers vs consumers)                │
│ - Compare types using TypeScript compiler                   │
│   * producerType.isAssignableTo(consumerType)              │
│ - Get TypeScript diagnostic messages                        │
│                                                             │
│ Output: type-check-results.json                            │
└─────────────────┬───────────────────────────────────────────┘
                  │
                  ↓
┌─────────────────────────────────────────────────────────────┐
│ Rust: Read Results & Include in Report                      │
│ - Parse type-check-results.json                            │
│ - Add type mismatches to ApiIssues                         │
│ - Generate final GitHub markdown report                     │
└─────────────────────────────────────────────────────────────┘
```

### 7.3 Type Naming Convention

**Critical for Matching:**

**Producer Types:**
```
{Method}{Path}ResponseProducer{RepoName}
```
Example: `GetApiUsersResponseProducerRepoA`

**Consumer Types:**
```
{Method}{Path}ResponseConsumer{CallId}{RepoName}
```
Example: `GetApiUsersResponseConsumerCall1RepoB`

**Path Encoding:**
- `/api/users` → `ApiUsers`
- `/users/:id` → `UsersById`
- `ENV_VAR:API_URL:/users` → `EnvVarApiUrlUsers`

### 7.4 Type Compatibility Checking

**Process:**

1. **Load All Type Files**:
   ```typescript
   const repoA = import("./repo-a_types");
   const repoB = import("./repo-b_types");
   ```

2. **Extract Type Aliases**:
   ```typescript
   // Find all types matching naming convention
   const producers = sourceFile.getTypeAliases()
     .filter(t => t.getName().includes("Producer"));
   ```

3. **Group by Endpoint**:
   ```typescript
   // GetApiUsersResponseProducerRepoA → "GET /api/users"
   const endpoint = parseTypeName(producerName);
   ```

4. **Compare Types**:
   ```typescript
   const producerType = producer.getType();
   const consumerType = consumer.getType();

   // Unwrap Response<T> if needed
   const unwrappedProducer = unwrapResponseType(producerType);

   // Check assignability
   const isCompatible = unwrappedProducer.isAssignableTo(consumerType);
   ```

5. **Get Error Details**:
   ```typescript
   if (!isCompatible) {
     // Create temp file with test assignment
     const tempCode = `
       declare const producer: ${producerType.getText()};
       const test: ${consumerType.getText()} = producer;
     `;

     // Get TypeScript compiler diagnostic
     const diagnostics = tempFile.getPreEmitDiagnostics();
     errorMessage = diagnostics[0].getMessageText();
   }
   ```

### 7.5 Integration Gap: Type Extraction from Agents

**Current State:**
- Type extraction only works with Gemini-analyzed async functions
- Not integrated with multi-agent results

**What's Needed:**

```rust
// In src/multi_agent_orchestrator.rs

impl MultiAgentOrchestrator {
    /// Extract types from agent results (NEW)
    fn extract_types_from_analysis(
        &self,
        analysis_results: &AnalysisResults,
    ) -> Vec<TypeInfo> {
        let mut type_infos = Vec::new();

        // Extract from endpoints
        for endpoint in &analysis_results.endpoints {
            // Parse handler function to find return type
            // Use AST or send to LLM for type extraction
            if let Some(type_ref) = self.extract_type_from_handler(&endpoint.handler) {
                type_infos.push(TypeInfo {
                    file_path: endpoint.location.split(':').next().unwrap(),
                    start_position: endpoint.type_position,
                    composite_type_string: type_ref,
                    alias: format!("{}{}ResponseProducer",
                                   endpoint.method,
                                   path_to_camel_case(&endpoint.path)),
                });
            }
        }

        // Extract from data calls
        for call in &analysis_results.data_fetching_calls {
            // Parse call site to find expected response type
            if let Some(type_ref) = self.extract_type_from_call(&call) {
                type_infos.push(TypeInfo {
                    file_path: call.location.split(':').next().unwrap(),
                    start_position: call.type_position,
                    composite_type_string: type_ref,
                    alias: format!("{}{}ResponseConsumerCall{}",
                                   call.method,
                                   path_to_camel_case(&call.target_url),
                                   call.call_id),
                });
            }
        }

        type_infos
    }
}
```

**Implementation Options:**

**Option 1: AST-Based Type Extraction**
- Use SWC to parse handler functions
- Find return type annotations
- Extract type from assignment expressions

**Option 2: LLM-Based Type Extraction**
- Send handler/call code to Gemini
- Ask LLM to extract type references with positions
- More accurate for complex cases

**Option 3: Dedicated TypeExtractionAgent**
- Create new agent specifically for type extraction
- Batch process endpoints and calls
- Return type references with positions

**Recommendation**: Start with Option 2 (LLM), optimize to Option 1 later.

---

## 8. Implementation Gaps

### 8.1 Gap Summary

| Gap | Priority | Effort | Blocker? |
|-----|----------|--------|----------|
| Zero output bug | 🚨 Critical | 2-4 hours | YES |
| Type extraction from agents | High | 4-6 hours | YES (for type checking) |
| Direct CloudRepoData build | Medium | 2-3 hours | No (adapter works) |
| Legacy code removal | Medium | 6-8 hours | No (can coexist) |
| Multi-framework testing | Medium | 4-6 hours | No (validation) |

### 8.2 Gap 1: Zero Output Bug (BLOCKER)

**Status**: 🚨 CRITICAL - Must fix first

**Problem**: Multi-agent system produces 0 endpoints and 0 calls

**Root Cause**: Likely agents not calling Gemini API

**Fix Approach:**
1. Add comprehensive debug logging
2. Verify Gemini API connectivity
3. Test with real API key (not "mock")
4. Isolate failure point (call sites, agents, mount graph, or conversion)
5. Fix specific broken component

**Estimated Effort**: 2-4 hours of debugging and fixing

**Success Criteria**:
- Running on `tests/fixtures/imported-routers` shows non-zero output
- Debug logs show data flowing through all stages
- At least 3 endpoints and 3 mounts detected

### 8.3 Gap 2: Type Extraction from Agent Results (BLOCKER for Type Checking)

**Status**: High priority, blocks type checking

**Problem**: Types only extracted from Gemini async function analysis, not from agent results

**Impact**: Type checking doesn't work with multi-agent endpoints/calls

**Fix Approach:**

**Phase 1: Modify Agent Schemas** (1-2 hours)
```rust
// Add type reference fields to agent outputs
HttpEndpoint {
    method: String,
    path: String,
    // NEW:
    response_type_file: Option<String>,
    response_type_position: Option<usize>,
    response_type_string: Option<String>,
}

DataFetchingCall {
    library: String,
    target_url: String,
    // NEW:
    expected_type_file: Option<String>,
    expected_type_position: Option<usize>,
    expected_type_string: Option<String>,
}
```

**Phase 2: Update LLM Prompts** (30 min)
- Ask agents to also extract type information
- Include type positions in Gemini responses

**Phase 3: Integrate with Type Extraction** (2-3 hours)
```rust
// In src/multi_agent_orchestrator.rs
let type_infos = self.extract_types_from_analysis(&analysis_results);

// Run type extraction
analyzer.extract_types_for_repo(repo_path, type_infos, packages)?;
```

**Estimated Effort**: 4-6 hours

**Success Criteria**:
- Type files generated from agent-extracted endpoints
- Cross-repo type checking works
- Type mismatches detected in test fixtures

### 8.4 Gap 3: Direct CloudRepoData Construction

**Status**: Medium priority, not a blocker

**Problem**: Using adapter pattern to convert multi-agent results to legacy format

**Current Code**:
```rust
// src/engine/mod.rs
fn convert_orchestrator_results_to_analyzer_data(
    result: &MultiAgentAnalysisResult,
) -> (Vec<ApiEndpointDetails>, Vec<ApiEndpointDetails>, ...) {
    // Convert ResolvedEndpoint → ApiEndpointDetails
    // Convert DataFetchingCall → ApiEndpointDetails
    // ...
}
```

**Desired Code**:
```rust
// Build CloudRepoData directly
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
            endpoints: mount_graph.get_resolved_endpoints(),
            calls: mount_graph.get_data_calls(),
            mounts: mount_graph.get_mounts(),
            // ...
        }
    }
}
```

**Why This Matters**: Cleaner architecture, one less conversion step

**Why It's Not Urgent**: Adapter works fine, no bugs

**Estimated Effort**: 2-3 hours

### 8.5 Gap 4: Legacy Code Removal

**Status**: Medium priority, improves maintainability

**Files to Remove/Refactor:**

1. **src/visitor.rs** - Express-specific visitors
   - Remove: `DependencyVisitor` Express pattern matching
   - Keep: Any helper functions still used

2. **src/extractor.rs** - Framework-specific extraction
   - Likely can be completely removed
   - Verify nothing else depends on it

3. **src/analyzer/mod.rs** - Parts using legacy patterns
   - Remove: Methods using `DependencyVisitor`
   - Keep: Type checking orchestration
   - Keep: Cross-repo analysis logic

**Search Patterns to Eliminate:**
```rust
// Find and remove:
.contains("express.Router()")
.contains("express()")
.contains("Router()")
// etc.
```

**Estimated Effort**: 6-8 hours

**Risk**: Medium (might break things if dependencies aren't clear)

**Mitigation**: Let tests guide you - if tests pass, removal was safe

### 8.6 Gap 5: Multi-Framework Testing

**Status**: Medium priority, validation step

**What's Needed**: Test fixtures for multiple frameworks

**Fixtures to Create:**

1. **Express** (already exists)
2. **Fastify**:
   ```typescript
   const app = fastify();
   app.register(routes, { prefix: '/api' });
   app.get('/users', async (req, reply) => { ... });
   ```

3. **Koa**:
   ```typescript
   const router = new Router();
   router.get('/users', async (ctx) => { ... });
   app.use(router.routes());
   ```

4. **Hapi**:
   ```typescript
   server.route({
     method: 'GET',
     path: '/users',
     handler: async (request, h) => { ... }
   });
   ```

5. **NestJS**:
   ```typescript
   @Controller('users')
   export class UsersController {
     @Get()
     async findAll() { ... }
   }
   ```

**Test Strategy**:
- Same API contract across all fixtures
- Verify same endpoints detected
- Verify same calls extracted
- Verify same type checking results

**Estimated Effort**: 4-6 hours (creating fixtures + tests)

---

## 9. Legacy Code Removal Strategy

### 9.1 Why Remove Legacy Code?

**Current Situation**: Two parallel analysis paths
1. Legacy (Express-specific) - works
2. Multi-agent (framework-agnostic) - broken

**Problems with Dual Paths:**
1. **Maintenance burden**: Two codebases to maintain
2. **Confusion**: Which path is used when?
3. **Divergence**: Changes to one might break the other
4. **Testing complexity**: Must test both paths

**Goal**: Single analysis path (multi-agent)

### 9.2 Safe Removal Process

**Step 1: Fix Multi-Agent System** ✅ Must do first
- Fix zero output bug
- Add type extraction from agents
- Verify all tests pass with multi-agent path

**Step 2: Create Multi-Framework Tests**
- Add test fixtures for Fastify, Koa, etc.
- Verify multi-agent handles them correctly
- Establish baseline for regression testing

**Step 3: Identify Dependencies**
```bash
# Find all usages of legacy components
rg "DependencyVisitor" --type rust
rg "\.visitor\(" --type rust
rg "express\.Router" --type rust
```

**Step 4: Remove in Phases**

**Phase 1: Remove Express Pattern Matching**
- Search for `.contains("express")` etc.
- Replace with behavior-based checks or remove entirely
- Run tests after each change

**Phase 2: Remove DependencyVisitor**
- Ensure all calls replaced with multi-agent equivalents
- Delete visitor implementations
- Run tests

**Phase 3: Clean Up Analyzer**
- Remove methods only used by legacy path
- Keep type checking and cross-repo logic
- Run tests

**Phase 4: Remove Adapter Functions**
- Build CloudRepoData directly from multi-agent results
- Delete `convert_orchestrator_results_to_analyzer_data()`
- Run tests

**Step 5: Update Documentation**
- Remove references to legacy approach
- Update architecture diagrams
- Update README

### 9.3 What to Keep

**Keep These Components** (framework-agnostic):
- `src/parser.rs` - SWC parsing
- `src/gemini_service.rs` - LLM integration
- `src/cloud_storage/` - AWS integration
- `src/formatter/` - Output formatting
- `src/config.rs` - Configuration
- `ts_check/` - Type checking system
- `src/analyzer/mod.rs` - Type checking coordination (not visitor usage)

**Remove These Components** (framework-specific):
- `src/visitor.rs` - Express visitors
- `src/extractor.rs` - Pattern-based extraction
- Framework pattern matching in `src/analyzer/`

### 9.4 Test Coverage for Safe Removal

**Before Removing:**
✅ All 43 tests pass with multi-agent path

**During Removal:**
✅ Run tests after each removal step
✅ If tests fail, investigate before proceeding

**After Removal:**
✅ All tests still pass
✅ Multi-framework fixtures all work
✅ Integration tests produce correct output

**Safety Net**: Output-focused tests don't care about implementation

---

## 10. Path to Completion

### 10.1 Phase 0: Fix Critical Bug (MANDATORY FIRST)

**Goal**: Get multi-agent system producing output

**Tasks:**

1. **Add Debug Logging** (30 min)
   - Log call site extraction count
   - Log agent invocation and results
   - Log mount graph construction
   - Log conversion layer results

2. **Test with Real API Key** (15 min)
   ```bash
   export CARRICK_API_KEY="sk-..."  # Real key from Google AI Studio
   cargo run -- tests/fixtures/imported-routers 2>&1 | tee debug.log
   ```

3. **Identify Failure Point** (1-2 hours)
   - Parse debug logs
   - Find where count drops to zero
   - Isolate specific broken component

4. **Fix Root Cause** (1-2 hours)
   - If CallSiteExtractor: Fix AST visitor
   - If Agents: Fix Gemini API calls
   - If MountGraph: Fix data flow
   - If Conversion: Fix mapping

5. **Validate Fix** (30 min)
   - Run on test fixtures
   - Verify non-zero output
   - Check output correctness

**Success Criteria:**
- ✅ Running on `tests/fixtures/imported-routers` shows endpoints/calls
- ✅ Debug logs show data flowing through all stages
- ✅ Output matches expected (3+ endpoints, 3+ mounts)

**Estimated Time**: 2-4 hours

### 10.2 Phase 1: Type Extraction Integration

**Goal**: Extract types from agent results, not just Gemini

**Tasks:**

1. **Modify Agent Schemas** (1 hour)
   - Add type reference fields to `HttpEndpoint`
   - Add type reference fields to `DataFetchingCall`
   - Update Gemini structured output schemas

2. **Update Agent Prompts** (30 min)
   - Ask EndpointAgent to extract response types
   - Ask ConsumerAgent to extract expected types
   - Test with sample code

3. **Implement Type Extraction** (2 hours)
   ```rust
   // In src/multi_agent_orchestrator.rs
   fn extract_types_from_analysis(&self, ...) -> Vec<TypeInfo> {
       // Extract from endpoints
       // Extract from calls
       // Return TypeInfo structs
   }
   ```

4. **Wire Through Pipeline** (1 hour)
   - Call type extraction after agent analysis
   - Pass to `analyzer.extract_types_for_repo()`
   - Verify type files generated

5. **Test Cross-Repo Type Checking** (1 hour)
   - Create test fixture with type mismatch
   - Run multi-agent analysis
   - Verify type mismatch detected

**Success Criteria:**
- ✅ Type files generated from agent results
- ✅ Cross-repo type checking works
- ✅ Type mismatches detected in tests

**Estimated Time**: 4-6 hours

### 10.3 Phase 2: Multi-Framework Validation

**Goal**: Prove framework agnosticism works

**Tasks:**

1. **Create Fastify Fixture** (1 hour)
   - Simple REST API with mounts
   - Same endpoints as Express fixture
   - Add to `tests/fixtures/`

2. **Create Koa Fixture** (1 hour)
   - Router-based API
   - Same endpoints

3. **Test Framework Detection** (30 min)
   - Verify correct framework detected
   - Check detection notes

4. **Test Endpoint Extraction** (30 min)
   - Run analysis on each fixture
   - Verify same endpoints found
   - Compare with Express baseline

5. **Create Comparison Test** (1 hour)
   ```rust
   #[tokio::test]
   async fn test_multi_framework_equivalence() {
       let express_result = analyze("fixtures/express-api");
       let fastify_result = analyze("fixtures/fastify-api");
       let koa_result = analyze("fixtures/koa-api");

       // All should find same endpoints
       assert_eq!(express_result.endpoints, fastify_result.endpoints);
       assert_eq!(express_result.endpoints, koa_result.endpoints);
   }
   ```

**Success Criteria:**
- ✅ All frameworks detected correctly
- ✅ Same endpoints extracted from equivalent fixtures
- ✅ Tests pass for all framework types

**Estimated Time**: 4-6 hours

### 10.4 Phase 3: Legacy Code Removal

**Goal**: Single analysis path, no framework-specific code

**Tasks:**

1. **Map Dependencies** (1 hour)
   ```bash
   rg "DependencyVisitor" --type rust > dependencies.txt
   # Analyze what still uses legacy code
   ```

2. **Remove Express Pattern Matching** (2 hours)
   - Find all `.contains("express")` etc.
   - Delete or replace with behavior-based checks
   - Run tests after each change

3. **Remove Visitor Code** (2 hours)
   - Delete `DependencyVisitor` implementation
   - Remove from `src/visitor.rs`
   - Update imports
   - Run tests

4. **Clean Up Analyzer** (2 hours)
   - Remove methods using legacy visitors
   - Keep type checking and cross-repo logic
   - Run tests

5. **Remove Adapter** (1 hour)
   - Build `CloudRepoData` directly from multi-agent results
   - Delete `convert_orchestrator_results_to_analyzer_data()`
   - Run tests

**Success Criteria:**
- ✅ No framework-specific pattern matching
- ✅ All tests still pass
- ✅ Code is cleaner and more maintainable

**Estimated Time**: 6-8 hours

### 10.5 Phase 4: Optimization and Polish

**Goal**: Production-ready multi-agent system

**Tasks:**

1. **Performance Benchmarking** (2 hours)
   - Measure LLM call counts
   - Measure execution time
   - Identify bottlenecks

2. **Optimize Batching** (1 hour)
   - Tune batch sizes
   - Adjust delays
   - Test with large repos

3. **Error Handling** (2 hours)
   - Add retry logic for agent failures
   - Handle partial results gracefully
   - Improve error messages

4. **Documentation** (2 hours)
   - Update README with multi-agent architecture
   - Document framework detection
   - Add examples for different frameworks

5. **Final Testing** (1 hour)
   - Run full test suite
   - Test on real repos
   - Verify CI passes

**Success Criteria:**
- ✅ Performance acceptable (< 30s for small repos)
- ✅ Robust error handling
- ✅ Clear documentation

**Estimated Time**: 6-8 hours

### 10.6 Total Effort Estimate

| Phase | Time | Priority |
|-------|------|----------|
| Phase 0: Fix Bug | 2-4 hours | 🚨 Critical |
| Phase 1: Type Extraction | 4-6 hours | High |
| Phase 2: Multi-Framework | 4-6 hours | Medium |
| Phase 3: Legacy Removal | 6-8 hours | Medium |
| Phase 4: Optimization | 6-8 hours | Low |
| **Total** | **22-32 hours** | **~1 week** |

---

## 11. Success Criteria

### 11.1 Functional Requirements

**Must Have:**

1. ✅ **Zero Output Bug Fixed**
   - Running analysis produces non-zero endpoints and calls
   - All stages of pipeline produce data

2. ✅ **Framework Agnosticism**
   - Works with Express, Fastify, Koa (minimum)
   - No hardcoded framework patterns
   - Same API contracts detected across frameworks

3. ✅ **Type Checking Integration**
   - Types extracted from agent results
   - Cross-repo type checking works
   - Type mismatches detected accurately

4. ✅ **Test Coverage Maintained**
   - All 43 existing tests pass
   - New multi-framework tests pass
   - Integration tests produce correct output

5. ✅ **Legacy Code Removed**
   - No Express-specific pattern matching
   - Single analysis path
   - Cleaner codebase

**Nice to Have:**

6. ⭐ **Performance**
   - Analysis completes in < 30 seconds for small repos
   - LLM usage optimized (< 50 calls per repo)

7. ⭐ **Additional Frameworks**
   - Hapi support
   - NestJS support
   - GraphQL support

### 11.2 Quality Metrics

**Code Quality:**
- No framework-specific string matching
- Single responsibility per component
- Clear data flow through pipeline

**Test Quality:**
- Output-focused (not implementation-focused)
- Fast (< 10 seconds)
- Deterministic

**Documentation Quality:**
- Architecture clearly explained
- Examples for each framework
- Debugging guide for common issues

### 11.3 Acceptance Test

**Scenario**: Analyze a multi-repo microservices system

**Given:**
- Repo A: Express-based user service
- Repo B: Fastify-based order service
- Repo C: Koa-based comment service
- All repos call each other's APIs

**When:**
- Run Carrick analysis on each repo

**Then:**
- ✅ All endpoints detected correctly (regardless of framework)
- ✅ All API calls detected correctly
- ✅ Cross-repo type checking works
- ✅ Missing endpoints identified
- ✅ Type mismatches identified
- ✅ Dependency conflicts identified
- ✅ Report generated successfully

---

## 12. Risk Assessment

### 12.1 Technical Risks

**Risk 1: LLM Accuracy**
- **Concern**: LLM might misclassify call sites
- **Likelihood**: Medium
- **Impact**: Medium (wrong categorization → missing endpoints)
- **Mitigation**:
  - Use structured output schemas
  - Provide framework context
  - Validate with multi-framework tests

**Risk 2: Performance**
- **Concern**: Too many LLM calls → slow/expensive
- **Likelihood**: Medium
- **Impact**: Medium (slow CI, high costs)
- **Mitigation**:
  - Batching (10 per request)
  - Parallel execution
  - Caching (future)

**Risk 3: Type Extraction Complexity**
- **Concern**: Extracting types from agent results is hard
- **Likelihood**: Low
- **Impact**: High (blocks type checking)
- **Mitigation**:
  - Use LLM to extract types (easier than AST parsing)
  - Structured output for type positions

**Risk 4: Breaking Changes**
- **Concern**: Removing legacy code breaks something unexpected
- **Likelihood**: Medium
- **Impact**: Medium (tests fail, rollback needed)
- **Mitigation**:
  - Remove incrementally
  - Run tests after each change
  - Output-focused tests catch regressions

### 12.2 Project Risks

**Risk 5: Scope Creep**
- **Concern**: Adding support for too many frameworks
- **Likelihood**: Medium
- **Impact**: Low (delays completion)
- **Mitigation**:
  - Start with Express, Fastify, Koa (3 frameworks)
  - Defer NestJS, Hapi, etc. to later

**Risk 6: Gemini API Changes**
- **Concern**: Gemini API or pricing changes
- **Likelihood**: Low
- **Impact**: High (system breaks)
- **Mitigation**:
  - Abstract LLM service (already done)
  - Easy to swap providers
  - Mock mode for testing

**Risk 7: Test Coverage Gaps**
- **Concern**: Tests don't catch edge cases
- **Likelihood**: Medium
- **Impact**: Medium (bugs in production)
- **Mitigation**:
  - Output-focused tests (already good)
  - Add multi-framework tests
  - Integration tests on real repos

### 12.3 Risk Mitigation Summary

**Overall Risk Level**: Medium

**Key Mitigations:**
1. Fix zero output bug first (reduces uncertainty)
2. Add comprehensive logging (improves debuggability)
3. Remove legacy code incrementally (reduces breaking change risk)
4. Multi-framework tests (validates agnosticism)
5. Output-focused test suite (enables safe refactoring)

---

## Conclusion

### Summary of Key Findings

1. **Vision is Excellent**: The multi-agent architecture is a sophisticated, well-thought-out solution for framework-agnostic API analysis.

2. **Critical Blocker**: The zero output bug must be fixed first. Everything else depends on it.

3. **Type Checking is Solid**: The `ts_check/` system is already framework-agnostic and well-designed. It just needs integration with agent results.

4. **Legacy Code Must Go**: The dual implementation path is unsustainable. Complete removal is necessary.

5. **Testing is Strong**: The 43-test suite with output-focused testing is perfect for validating this refactor.

6. **Achievable Goal**: With focused effort (22-32 hours), this transformation is completely achievable.

### What Makes This Architecture Special

**True Framework Agnosticism**:
- No hardcoded patterns
- LLM-powered semantic understanding
- Works with any framework automatically

**Sophisticated Type Safety**:
- TypeScript compiler for validation
- Cross-repository type checking
- Catches incompatibilities before deployment

**Optimized LLM Usage**:
- Classify-Then-Dispatch reduces costs
- Batching prevents timeouts
- Parallel execution improves speed

**Clean Architecture**:
- Single analysis path
- Clear separation of concerns
- Framework-agnostic components

### Next Steps

1. **Immediate**: Fix zero output bug (Phase 0)
2. **Short-term**: Integrate type extraction (Phase 1)
3. **Medium-term**: Validate with multiple frameworks (Phase 2)
4. **Long-term**: Remove legacy code (Phase 3)
5. **Polish**: Optimize and document (Phase 4)

### Final Recommendation

**This transformation is not only feasible but necessary.** The multi-agent architecture represents the correct long-term design for Carrick. The current dual-path situation is technical debt that will only grow over time.

**Priority**: Fix the critical zero output bug immediately, then proceed with type extraction integration. These two fixes will unblock everything else.

**Timeline**: With focused effort, this can be completed in approximately one week (22-32 hours of work).

**Confidence Level**: High - The architecture is sound, the testing infrastructure is solid, and the path forward is clear.

---

**Document End**

**Author**: Comprehensive analysis synthesizing multi-agent-workflow research with existing project documentation
**Date**: November 16, 2025
**Version**: 1.0
**Branch**: multi-agent
