# File-Centric Analysis Architecture

**Status:** Implementation In Progress  
**Last Updated:** 2024-01

## Overview

This document describes the refactored Carrick Static Analysis Engine architecture that uses a file-centric approach with Gemini 3.0 Flash for high-speed, framework-agnostic code analysis.

## Motivation

The previous "Batch-of-10" architecture had several limitations:

1. **High Token Cost**: Batching call sites required multiple LLM round-trips
2. **Poor Context**: Splitting files across batches broke alias resolution
3. **Complex Orchestration**: Triage → Dispatch flow added latency and complexity
4. **Hardcoded Framework Knowledge**: System prompts contained framework-specific logic

## New Architecture

### Core Principle: Framework Agnosticism

The system must be **Strictly Framework Agnostic**. All detection logic is derived dynamically from injected patterns, with no hardcoded references to specific libraries (e.g., Express, Fastify) in the system prompts.

### Analysis Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                    Old Flow (Deprecated)                        │
├─────────────────────────────────────────────────────────────────┤
│  Regex → Batch (10 sites) → LLM Triage → LLM Dispatch          │
│  • High token cost                                              │
│  • Poor context for alias resolution                            │
│  • Multiple LLM round-trips                                     │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                    New Flow (File-Centric)                      │
├─────────────────────────────────────────────────────────────────┤
│  1. Read File      → Load full content of target_file.ts       │
│  2. Load Guidance  → Retrieve FrameworkGuidance patterns       │
│  3. One-Shot       → Send File + Patterns to Gemini            │
│  4. Direct Build   → Deserialize JSON into MountGraph structs  │
└─────────────────────────────────────────────────────────────────┘
```

### Key Components

#### 1. FileAnalyzerAgent (`src/agents/file_analyzer_agent.rs`)

The core analysis agent that processes one file at a time:

- **Input**: File path, file content, FrameworkGuidance patterns
- **Output**: `FileAnalysisResult` containing mounts, endpoints, and data_calls
- **System Prompt**: Framework-agnostic, relies strictly on provided patterns

```rust
pub struct FileAnalysisResult {
    pub mounts: Vec<MountResult>,
    pub endpoints: Vec<EndpointResult>,
    pub data_calls: Vec<DataCallResult>,
}
```

#### 2. FileOrchestrator (`src/agents/file_orchestrator.rs`)

Coordinates file-centric analysis across multiple files:

- Processes files sequentially or in parallel
- Aggregates results into a unified `MountGraph`
- Handles cross-file resolution via `import_source` tracking
- Provides processing statistics and error reporting

#### 3. Flat Output Schema (`src/agents/schemas.rs`)

Uses a flat JSON schema to avoid recursion errors and ensure deterministic parsing:

```json
{
  "mounts": [...],
  "endpoints": [...],
  "data_calls": [...]
}
```

### Cross-File Resolution

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

## System Prompt Design

The system prompt emphasizes:

1. **Pattern Matching**: Only extract what matches provided patterns
2. **No Hallucinations**: Don't infer from comments or vague code
3. **Alias Resolution**: Track imports and resolve variable definitions
4. **Flat Output**: All findings as top-level items in respective lists

Key sections:
- `CORE OBJECTIVE`: Scan and extract based on patterns
- `ANALYSIS RULES`: Strict pattern matching, variable resolution
- `OUTPUT REQUIREMENTS`: Flat schema, exact literals
- `IMPORT TRACKING`: How to record import sources

## Benefits

1. **Better Context**: Full file content enables accurate alias resolution
2. **Lower Token Cost**: One LLM call per file vs. multiple for batching
3. **Framework Agnostic**: Patterns injected at runtime, not hardcoded
4. **Deterministic**: Flat schema ensures consistent JSON parsing
5. **Simpler Flow**: Direct file → result mapping

## Testing Strategy

1. **Unit Tests**: Schema validation, serialization, pattern formatting
2. **Integration Tests**: Mock agent responses, cross-file resolution
3. **Edge Cases**: Empty files, missing files, nested mounts

## Migration Path

The file-centric architecture coexists with the existing batch-based system:

1. ✅ Add FileAnalyzerAgent and schema
2. ✅ Add FileOrchestrator for file processing
3. ✅ Add mock response generation for testing
4. ✅ Add comprehensive tests
5. ⬜ Integrate with main analysis pipeline (optional feature flag)
6. ⬜ Performance benchmarking against batch approach
7. ⬜ Gradual migration with fallback support

## Files Added

- `src/agents/file_analyzer_agent.rs` - File-centric analysis agent
- `src/agents/file_orchestrator.rs` - Multi-file orchestration
- `src/agents/schemas.rs` - Extended with `file_analysis_schema()`
- `tests/file_centric_analysis_test.rs` - Integration tests
- `docs/research/file-centric-analysis-architecture.md` - This document