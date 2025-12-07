# Carrick Project Documentation

**Last Updated**: January 2025  
**Status**: Multi-agent framework-agnostic architecture implemented  
**Tests**: 70+ Passing | **Clippy**: Clean | **Frameworks**: Express, Fastify, Koa

---

## What is Carrick?

Carrick is a **cross-repository API consistency analysis tool** that detects mismatches between HTTP producers (endpoints) and consumers (outbound calls) across microservices.

### Core Capabilities

1. **Extracts endpoints** from services (Express, Fastify, Koa, etc.)
2. **Extracts outbound HTTP calls** (`fetch`, `axios`, etc.)
3. **Matches calls to endpoints** across repositories
4. **Detects type mismatches** between producer and consumer
5. **Analyzes dependency conflicts** across repos

### Key Achievement

The system is **purely framework-agnostic** - all Express-specific pattern matching has been removed. Analysis uses the mount graph and LLM-based behavior classification.

---

## Architecture Overview

### Data Flow

```
analyze_current_repo()
    ↓
MultiAgentOrchestrator::run_complete_analysis()
    ↓
MultiAgentAnalysisResult (contains MountGraph)
    ↓
CloudRepoData::from_multi_agent_results()
    ↓
CloudRepoData (with mount_graph field)
    ↓
[Serialized to cloud storage]
    ↓
run_analysis_engine() downloads all repos
    ↓
build_cross_repo_analyzer() merges data + mount graphs
    ↓
Analyzer::get_results() → uses mount_graph methods
    ↓
print_results()
```

### Multi-Agent Pipeline

```
Stage 0: Framework Detection
  └─> Detected: ["express"], ["axios"]

Stage 1: Call Site Extraction (Universal AST traversal)
  └─> Extracted: Call sites from all files

Stage 2: Triage Classification (LLM-based)
  └─> Classified: HttpEndpoint, DataFetchingCall, RouterMount, Middleware, Irrelevant

Stage 3: Specialist Agent Dispatch (Parallel)
  ├─> EndpointAgent: Extracts endpoint details
  ├─> ConsumerAgent: Extracts API call details
  ├─> MountAgent: Extracts router mount relationships
  └─> MiddlewareAgent: Extracts middleware

Stage 4: Mount Graph Construction
  └─> Built: Nodes, mounts, endpoints with resolved paths

Stage 5: Type Extraction & Cross-Repo Type Checking
  └─> TypeScript-based type compatibility validation
```

---

## Work Completed

### ✅ Phase 0: Fix Critical Zero Output Bug

**Problem**: Multi-agent system produced 0 endpoints/calls despite files being discovered.

**Solution**: Created schema-aware mock response generator that inspects response schema and generates appropriate mock responses per agent type.

### ✅ Phase 1: Type Extraction Integration

- Added `extract_types_from_analysis()` to MultiAgentOr
chestrator
- Integrated with `ts_check/` TypeScript type checking system

### ✅ Phase 2: Legacy Code Removal

**Priority 1 - Adapter Layer Removal**:
- Created `CloudRepoData::from_multi_agent_results()` for direct construction
- Removed adapter conversion function (~60 lines)

**Priority 2 - Legacy Analysis Methods**:
- Deleted `analyze_matches()`, `find_matching_endpoint()`, `compare_calls_to_endpoints()` (341 lines)
- Added mount graph-based analysis methods
- Added `MountGraph::merge_from_repos()` for cross-repo analysis

**Priority 3 - DependencyVisitor Simplification**: 🟡 Deferred (system works without it)

### ✅ Phase 3: Multi-Framework Testing

Validated: Express, Fastify, Koa

### ✅ Phase 4: URL Normalization

- Created `src/url_normalizer.rs` (650 lines)
- Handles full URLs, template literals, env vars
- Enhanced path matching with optional segments and wildcards
- Integrated with `carrick.json` domain configuration

---

## Key Design Decisions

1. **Framework Agnostic**: All analysis via LLM behavior classification, no framework-specific patterns
2. **No Backwards Compatibility**: `get_results()` requires mount graph
3. **Mount Graph as Single Source of Truth**: All endpoint/call matching via mount graph
4. **TypeScript for Type Checking**: Uses actual TS compiler via `ts_check/`

---

## Test Coverage

| Test Suite | Count | Status |
|------------|-------|--------|
| Unit tests | 11+ | ✅ |
| Integration tests | 3 | ✅ |
| Multi-agent tests | 4+ | ✅ |
| Multi-framework tests | 1 | ✅ |
| Dependency analysis | 4 | ✅ |
| Mock storage | 10 | ✅ |
| Output contract | 4 | ✅ |
| URL normalizer | 19 | ✅ |
| Mount graph matching | 6 | ✅ |
| **Total** | **70+** | ✅ |

**Philosophy**: Output-focused testing - tests verify results, not implementation.

---

## Key Files

### Core Components

| File | Purpose |
|------|---------|
| `src/multi_agent_orchestrator.rs` | Orchestrates multi-agent pipeline |
| `src/mount_graph.rs` | Framework-agnostic endpoint/call/mount graph |
| `src/url_normalizer.rs` | URL normalization for matching |
| `src/cloud_storage/mod.rs` | CloudRepoData with mount_graph field |
| `src/analyzer/mod.rs` | Analysis logic using mount graph |
| `src/engine/mod.rs` | Main analysis engine |
| `ts_check/` | TypeScript type checking system |

### Agent System

| File | Purpose |
|------|---------|
| `src/agents/orchestrator.rs` | Classify-then-dispatch logic |
| `src/agents/triage_agent.rs` | Classifies call sites |
| `src/agents/endpoint_agent.rs` | Extracts HTTP endpoints |
| `src/agents/consumer_agent.rs` | Extracts API calls |
| `src/agents/mount_agent.rs` | Extracts router mounts |
| `src/agents/middleware_agent.rs` | Extracts middleware |

---

## Running the Project

```bash
# Set required environment variable
export CARRICK_API_ENDPOINT=http://localhost:8000

# Run all tests
cargo test

# Check code quality
cargo fmt --check
cargo clippy --all-targets -- -D warnings

# Run with mock mode (no Gemini API calls)
CARRICK_MOCK_ALL=1 cargo run -- --local /path/to/repo

# Run with real Gemini API
GEMINI_API_KEY=your-key cargo run -- --local /path/to/repo
```

---

## Configuration

### carrick.json Example

```json
{
  "internalDomains": ["user-service.internal", "api.internal"],
  "externalDomains": ["api.stripe.com"],
  "internalEnvVars": ["USER_SERVICE_URL", "API_BASE_URL"],
  "externalEnvVars": ["STRIPE_API_URL"]
}
```

---

## Remaining Work

### Optional: DependencyVisitor Simplification (2-3 hours)
- Remove endpoint/call/mount extraction from `DependencyVisitor`
- System works fine without this

### Test Infrastructure
- Add periodic integration tests with real Gemini API
- Create deterministic fixtures with pre-computed responses

---

## Research Documents

Detailed reference documentation in `research/`:

| Document | Purpose |
|----------|---------|
| [cloud_infrastructure.md](research/cloud_infrastructure.md) | AWS architecture (S3, DynamoDB, Lambda) |
| [ts_check.md](research/ts_check.md) | TypeScript type checking system |
| [testing_strategy.md](research/testing_strategy.md) | Testing philosophy and coverage |

---

## Quick Reference

| I want to... | Do this |
|--------------|---------|
| Run tests | `CARRICK_API_ENDPOINT=http://localhost:8000 cargo test` |
| Add a test | See `research/testing_strategy.md` |
| Understand type checking | See `research/ts_check.md` |
| Understand cloud storage | See `research/cloud_infrastructure.md` |
| Debug issues | Add `RUST_LOG=debug` and run with `--nocapture` |