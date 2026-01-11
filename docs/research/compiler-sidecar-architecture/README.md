# Compiler Sidecar Architecture - Quick Reference

> **TL;DR**: Replace fragile position-based type extraction with a warm Node.js process that uses the actual TypeScript compiler—including implicit type inference for unannotated code.

## Core Design Principles

1. **REST-Based, Framework/Library Agnostic** - Works with any TypeScript HTTP framework (Express, Fastify, Hono, tRPC, Koa, custom)
2. **Parallel Startup** - Sidecar spawns IMMEDIATELY at CLI start; SWC and LLM proceed while TypeScript initializes
3. **Implicit Type Inference** - Extract types even when developers don't write explicit annotations
4. **CI-First** - Fast, deterministic execution in CI pipelines (no added latency from sidecar)

## The Problem

The current type extraction pipeline has fundamental issues:

| Issue | Impact |
|-------|--------|
| LLM provides line numbers, but types span multiple lines | Type position lookup fails ~30% of the time |
| SWC visitor pattern is complex | Hard to maintain, edge cases everywhere |
| Alias naming conventions drive type matching | Regex parsing is brittle, new patterns break |
| Manual dependency traversal | Misses transitive types, over-collects others |

## The Solution

### Parallel Startup Timeline

```
CLI Start ─────────────────────────────────────────────────────► S3 Upload
    │
    ├──► Spawn Sidecar (async) ──────────────────────┐
    │         │                                      │
    │         └──► TS Project init (~500ms)          │
    │                                                │
    ├──► SWC Scanning (parallel) ─────────┐          │
    │                                     │          │
    ├──► LLM Analysis (parallel) ─────────┤          │
    │                                     │          │
    └──► Type Resolution (after both) ────┴──────────┘
                                                     
    Total time = MAX(sidecar_init, swc+llm), NOT the SUM
```

### Architecture

```
┌─────────────────────┐     JSON/stdin      ┌─────────────────────┐
│   Rust CLI          │ ─────────────────── │   Node.js Sidecar   │
│   (Orchestrator)    │                     │   (ts-morph +       │
│                     │ ◄───────────────── │    dts-bundle-gen)  │
└─────────────────────┘     JSON/stdout     └─────────────────────┘
         │                                            │
         │  1. Gemini extracts:                      │  3. Sidecar:
         │     - symbol: "User" (explicit)           │     - Bundles explicit types
         │     - OR line_number (for inference)      │     - Infers implicit types
         │                                           │     - Returns flat .d.ts
         │  2. Rust creates request ─────────────────┘
         │
         ▼
    ┌─────────────────────┐
    │  Bundled types.d.ts │  ← All types (explicit + inferred)
    └─────────────────────┘
```

## Key Benefits

1. **No position lookup** - Just need symbol name and import source
2. **Compiler-accurate** - TypeScript itself resolves all dependencies
3. **Implicit type inference** - Extract types even without annotations (major!)
4. **Parallel startup** - No added CI latency (sidecar init overlaps with SWC/LLM)
5. **Framework agnostic** - Works with Express, Fastify, Koa, Hono, tRPC, etc.
6. **Flattened output** - Single .d.ts file with everything bundled

## Quick Start

### Read the Docs

1. **[ARCHITECTURE.md](./ARCHITECTURE.md)** - Full technical design
2. **[IMPLEMENTATION_PLAN.md](./IMPLEMENTATION_PLAN.md)** - Step-by-step prompts

### Key Files (After Implementation)

```
carrick/
├── src/
│   ├── sidecar/                    # NEW: Node.js type bundler
│   │   ├── src/index.ts            # Message loop entry point
│   │   ├── src/bundler.ts          # dts-bundle-generator wrapper
│   │   └── src/project-loader.ts   # ts-morph project init
│   └── services/
│       └── type_sidecar.rs         # NEW: Rust process manager
```

### Message Protocol

**Init Request:**
```json
{"action": "init", "request_id": "1", "repo_root": "/path/to/repo"}
```

**Bundle Request (Explicit Types):**
```json
{
  "action": "bundle",
  "request_id": "2", 
  "symbols": [
    {"symbol_name": "User", "source_file": "./types/user.ts", "alias": "GetUsersResponseProducer"}
  ]
}
```

**Infer Request (Implicit Types - NEW!):**
```json
{
  "action": "infer",
  "request_id": "3",
  "infer_requests": [
    {"file_path": "./routes/users.ts", "line_number": 15, "infer_kind": "response_body", "alias": "GetUsersResponse"}
  ]
}
```

**Response:**
```json
{
  "request_id": "2",
  "status": "success",
  "dts_content": "export interface User { id: string; name: string; }\nexport type GetUsersResponseProducer = User[];",
  "inferred_types": [
    {"alias": "GetUsersResponse", "type_string": "User[]", "is_explicit": false}
  ]
}
```

## Migration Path

| Phase | What | Flag |
|-------|------|------|
| 1 | Build sidecar, run in parallel | `--sidecar-type-extraction` |
| 2 | Compare old vs new output | Both enabled |
| 3 | Make sidecar default | `--legacy-type-extraction` for old |
| 4 | Remove legacy code | N/A |

## FAQ

**Q: Why not just fix the position-based approach?**  
A: We've tried (see branch history). The fundamental issue is that LLMs provide semantic information (type names) while position-based extraction needs syntactic information (byte offsets). Converting between them is inherently lossy.

**Q: Why a sidecar process instead of calling Node.js per-file?**  
A: TypeScript project initialization is expensive (~500ms). A warm sidecar amortizes this across all files. Plus, parallel startup means this cost is hidden behind SWC/LLM work.

**Q: What about implicit types—how does that work?**  
A: The TypeScript compiler infers types even when developers don't write annotations. For `res.json(users)`, TypeScript knows `users` is `User[]`. The sidecar asks TypeScript "what's the type at this location?" and gets back the inferred type.

**Q: How is this framework-agnostic?**  
A: Instead of hardcoding "Express uses res.json()", we look for common patterns: `.json()` method calls, `ctx.body` assignments, `return` statements, etc. The sidecar doesn't know or care if it's Express, Fastify, Koa, or Hono.

**Q: What about dts-bundle-generator alternatives?**  
A: We could use ts-morph's emit, but dts-bundle-generator handles more edge cases (external types, circular deps). We can swap implementations inside the sidecar without changing the Rust interface.

**Q: Will this slow down CI?**  
A: No! The sidecar spawns immediately and initializes in parallel with SWC scanning and LLM analysis. By the time we need types, the sidecar is already ready. Total time = MAX(sidecar_init, swc+llm), not the SUM.

**Q: Will this break CI?**  
A: CI needs Node.js 18+ and npm. The sidecar is built as part of `cargo build` (or separate npm script).