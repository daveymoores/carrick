# File-Centric Analysis Architecture

**Status:** Implemented (AST-Gated)  
**Last Updated:** 2025-01

## Overview

This document describes the refactored Carrick Static Analysis Engine architecture that uses an **AST-Gated File-Centric** approach with Gemini 3.0 Flash for high-speed, framework-agnostic code analysis.

## Motivation

The previous "Batch-of-10" architecture had several limitations:

1. **High Token Cost**: Batching call sites required multiple LLM round-trips
2. **Poor Context**: Splitting files across batches broke alias resolution
3. **Complex Orchestration**: Triage → Dispatch flow added latency and complexity
4. **Hardcoded Framework Knowledge**: System prompts contained framework-specific logic

## Architecture Evolution

```
┌─────────────────────────────────────────────────────────────────┐
│                    Old Flow (REMOVED)                           │
├─────────────────────────────────────────────────────────────────┤
│  Regex → Batch (10 sites) → LLM Triage → LLM Dispatch          │
│  • High token cost                                              │
│  • Poor context for alias resolution                            │
│  • Multiple LLM round-trips                                     │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                Current Flow (AST-Gated File-Centric)            │
├─────────────────────────────────────────────────────────────────┤
│  1. SWC AST Scan  → Gatekeeper: find candidate call sites      │
│     └─ If NO candidates → SKIP file (ZERO LLM cost)            │
│  2. Read File     → Load full content of target_file.ts        │
│  3. Load Guidance → Retrieve FrameworkGuidance patterns        │
│  4. Inject Hints  → Include Candidate Targets in LLM prompt    │
│  5. One-Shot LLM  → Send File + Patterns + Hints to Gemini     │
│  6. Direct Build  → Deserialize JSON into MountGraph structs   │
└─────────────────────────────────────────────────────────────────┘
```

## Core Principles

### 1. Framework Agnosticism

The system is **Strictly Framework Agnostic**. All detection logic is derived dynamically from injected patterns, with no hardcoded references to specific libraries (e.g., Express, Fastify) in the system prompts.

### 2. AST Gatekeeper (Zero-Cost Filtering)

The SWC Scanner acts as a cheap gatekeeper:
- **Purpose**: Identify files that *might* contain API patterns before invoking the LLM
- **Cost**: Near-zero (fast AST parse, no network call)
- **Benefit**: Files with no candidates are skipped entirely—no LLM tokens consumed

### 3. Candidate-Focused LLM Analysis

When a file passes the gatekeeper:
- Full file content is sent to the LLM (for alias/import resolution)
- Candidate line numbers are injected as "hints" to focus the LLM's attention
- The LLM acts as a Pattern Matcher + Alias Resolver, not a general code analyzer

## Key Components

### 1. SWC Scanner / Gatekeeper (`src/swc_scanner.rs`)

A lightweight AST-based scanner that identifies potential API call sites:

```rust
pub struct CandidateTarget {
    pub line_number: u32,
    pub callee_object: Option<String>,  // e.g., "app", "router", "axios"
    pub callee_property: Option<String>, // e.g., "get", "post", "use"
    pub code_snippet: String,            // The actual code at this line
}
```

**Detection rules** (intentionally broad to avoid false negatives):
- Method calls on `app`, `router`, `axios`, `fetch`, `res`, etc.
- Chained calls (e.g., `express().use(...)`)
- Custom router names and HTTP method patterns

### 2. FileAnalyzerAgent (`src/agents/file_analyzer_agent.rs`)

The core analysis agent that processes one file at a time:

- **Input**: File path, file content, FrameworkGuidance patterns, Candidate Targets
- **Output**: `FileAnalysisResult` containing mounts, endpoints, and data_calls
- **System Prompt**: Framework-agnostic, relies strictly on provided patterns
- **Method**: `analyze_file_with_candidates(file_path, file_content, guidance, candidate_hints)`

```rust
pub struct FileAnalysisResult {
    pub mounts: Vec<MountResult>,
    pub endpoints: Vec<EndpointResult>,
    pub data_calls: Vec<DataCallResult>,
}
```

### 3. FileOrchestrator (`src/agents/file_orchestrator.rs`)

Coordinates file-centric analysis across multiple files:

- Runs the SWC Scanner on each file (gatekeeper)
- Skips files with no candidates (tracks `files_skipped_no_candidates`)
- For files with candidates: formats hints and calls FileAnalyzerAgent
- Aggregates results into a unified `MountGraph`
- Handles cross-file resolution via `import_source` tracking

```rust
pub struct ProcessingStats {
    pub files_processed: usize,
    pub files_skipped_no_candidates: usize,  // Zero LLM cost
    pub files_with_errors: usize,
    pub total_mounts: usize,
    pub total_endpoints: usize,
    pub total_data_calls: usize,
}
```

### 4. Flat Output Schema (`src/agents/schemas.rs`)

Uses a flat JSON schema to avoid recursion errors and ensure deterministic parsing:

```json
{
  "mounts": [...],
  "endpoints": [...],
  "data_calls": [...]
}
```

## LLM Prompt Structure

The FileAnalyzerAgent prompt includes:

1. **INPUT DATA Section**:
   - Full Source Code (complete file content)
   - Candidate Targets (SWC-detected line hints)
   - Active Patterns (framework-specific patterns from guidance)

2. **Instructions**:
   - Focus on Candidate Targets but use full file context for alias resolution
   - Match only against Active Patterns
   - Resolve variable definitions and imports
   - Filter noise (skip candidates that don't match patterns)

3. **Output Requirements**:
   - Flat JSON schema (mounts, endpoints, data_calls arrays)
   - Exact string literals (no inferred paths)
   - Include `import_source` for cross-file linking

Example candidate hints in prompt:
```
CANDIDATE TARGETS (SWC-detected lines to focus on):
- Line 12: app.use - `app.use('/api', apiRouter)`
- Line 25: router.get - `router.get('/users', getUsers)`
```

## Cross-File Resolution

The key innovation is the `import_source` field in mount results:

```rust
pub struct MountResult {
    pub line_number: i32,
    pub parent_node: String,    // e.g., "app"
    pub child_node: String,     // e.g., "userRouter"
    pub mount_path: String,     // e.g., "/users"
    pub import_source: Option<String>, // e.g., "./routes/users"
    pub pattern_matched: String,
}
```

When the LLM analyzes:
```typescript
import userRouter from './routes/users';
app.use('/users', userRouter);
```

It records `import_source: Some("./routes/users")`, which allows the orchestrator to link endpoints defined in `routes/users.ts` to the `/users` prefix.

## Benefits

1. **Zero-Cost Filtering**: Files without API patterns skip LLM entirely
2. **Better Context**: Full file content enables accurate alias resolution
3. **Focused Analysis**: Candidate hints improve LLM accuracy and reduce hallucination
4. **Lower Token Cost**: One LLM call per relevant file vs. multiple for batching
5. **Framework Agnostic**: Patterns injected at runtime, not hardcoded
6. **Deterministic**: Flat schema ensures consistent JSON parsing
7. **Simpler Flow**: Direct file → result mapping (no triage/dispatch)

## Testing Strategy

1. **Unit Tests**: Schema validation, serialization, pattern formatting, SWC scanner detection
2. **Integration Tests**: Mock agent responses, cross-file resolution
3. **Edge Cases**: Empty files, missing files, nested mounts, files with no candidates

## Implementation Status

### Completed

1. ✅ SWC Scanner (AST Gatekeeper) - `src/swc_scanner.rs`
2. ✅ FileAnalyzerAgent with candidate hints support
3. ✅ FileOrchestrator with AST gating integration
4. ✅ Flat output schema (`file_analysis_schema()`)
5. ✅ Mock response generation for testing
6. ✅ Comprehensive unit and integration tests
7. ✅ Removal of old Batch-of-10 orchestration (TriageAgent, CallSiteOrchestrator, etc.)
8. ✅ Integration with MultiAgentOrchestrator

### Remaining Work

1. ⬜ Import source extraction via SWC (for automatic cross-file linking)
2. ⬜ End-to-end validation with real Gemini 3.0 Flash (not mock)
3. ⬜ Performance/cost metrics telemetry
4. ⬜ Remove `legacy_types.rs` after full migration of dependent code

## Files

### Added/Modified for AST-Gated Architecture

- `src/swc_scanner.rs` - SWC-based AST gatekeeper
- `src/agents/file_analyzer_agent.rs` - File-centric analysis agent with candidate hints
- `src/agents/file_orchestrator.rs` - Multi-file orchestration with AST gating
- `src/agents/schemas.rs` - Extended with `file_analysis_schema()`
- `src/agents/multi_agent_orchestrator.rs` - Updated to use file-centric flow
- `src/agents/legacy_types.rs` - Compatibility types for transition
- `tests/file_centric_analysis_test.rs` - Integration tests

### Removed (Old Batch Architecture)

- `src/agents/triage_agent.rs` - Old triage agent
- `src/agents/call_site_orchestrator.rs` - Old batch orchestrator
- `src/agents/consumer_agent.rs` - Old dispatch agent
- `src/agents/endpoint_agent.rs` - Old dispatch agent
- `src/agents/middleware_agent.rs` - Old dispatch agent
- `src/agents/mount_agent.rs` - Old dispatch agent

## Architecture Diagram

```
                    ┌──────────────────┐
                    │   Source Files   │
                    └────────┬─────────┘
                             │
                             ▼
                    ┌──────────────────┐
                    │   SWC Scanner    │  ◄── Gatekeeper (ZERO LLM cost)
                    │  (AST Analysis)  │
                    └────────┬─────────┘
                             │
              ┌──────────────┴──────────────┐
              │                             │
              ▼                             ▼
     ┌────────────────┐           ┌────────────────┐
     │  No Candidates │           │ Has Candidates │
     │    (SKIP)      │           │                │
     └────────────────┘           └───────┬────────┘
                                          │
                                          ▼
                                 ┌────────────────┐
                                 │ FileAnalyzer   │
                                 │    Agent       │
                                 │ (Gemini 3.0)   │
                                 └───────┬────────┘
                                         │
                                         ▼
                                 ┌────────────────┐
                                 │ FileAnalysis   │
                                 │    Result      │
                                 └───────┬────────┘
                                         │
                                         ▼
                                 ┌────────────────┐
                                 │  MountGraph    │
                                 │  (Aggregated)  │
                                 └────────────────┘
```

## Configuration

The AST-gated file-centric analysis is now the **default and only** analysis mode. The old batch-based flow has been removed.

LLM Configuration:
- **Model**: Gemini 3.0 Flash
- **Temperature**: 0.0 (deterministic)
- **Response Schema**: Strict flat JSON schema enforced