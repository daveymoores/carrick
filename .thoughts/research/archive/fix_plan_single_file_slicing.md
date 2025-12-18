# Fix Plan: Single-File Static Slicing via Use–Def Chains (SWC)

**Created**: December 2025  
**Status**: Proposed / Planning  
**Priority**: High  
**Last Updated**: December 2025

---

## Executive Summary

Carrick currently extracts call sites from JavaScript/TypeScript ASTs and relies on brittle pattern matching in places to infer dynamic constructs (e.g., routes built through loops or helper functions).

This plan replaces pattern-specific logic with a code-agnostic **single-file static slicing** approach based on a **Use–Def chain**:

- When an API definition call is encountered (e.g., `app.get(route, ...)`), we trace identifiers used in the relevant argument back to their **definitions within the same file**.
- We emit a minimal, ordered snippet of source code (`context_slice`) that contains exactly the local definitions (and import boundaries) needed for an LLM to infer the value/shape of the argument.

Key constraint: **single-file only** (no cross-file symbol resolution).

---

## Scope and Non-Goals

### In scope
- Single-file Use–Def resolution for identifiers used in API call arguments.
- Definitions discovered from:
  - local variable declarations
  - imports (boundary)
  - callback parameters introduced by call-structure (e.g., `.forEach(x => ...)`, `.map(x => ...)`, etc.)
- Output: a `context_slice` string on `CallSite` containing relevant source snippets sorted by line order.

### Non-goals
- Cross-file/module resolution.
- Full constant evaluation or execution.
- General-purpose dataflow analysis across control-flow graphs.

---

## Architecture Overview

This feature is implemented per file using three phases:

1. **Phase 0 (Mandatory): Resolver pass**
2. **Phase 1: Indexing pass (build a DefinitionIndex)**
3. **Phase 2: Slicing (compute context_slice per call site)**

The resolver pass is a hard requirement to ensure identifiers are uniquely keyed by `(Symbol, SyntaxContext)`.

---

## Phase 0 (Mandatory): SWC Resolver

### Why the resolver is required
Without `swc_ecma_transforms_base::resolver`, identifiers are effectively just strings, and unrelated bindings can collide (e.g., `i` in a loop vs `i` at module scope). Any Use–Def chain built without the resolver can link the wrong definitions, producing garbage slices.

### Plan
- After parsing a file into a `swc_ecma_ast::Module`, run the SWC resolver transform.
- All subsequent phases operate on the **resolved** AST.

### Invariants
- All `DefinitionId` keys are computed from the resolved AST.
- All identifier uses are resolved to unique `swc_ecma_ast::Id` values (`(sym, SyntaxContext)`).

---

## Data Structures (to add in `src/call_site_extractor.rs`)

### 1) DefinitionIndex

```rust
type DefinitionId = swc_ecma_ast::Id;

struct DefinitionIndex {
    /// Map an identifier to the definition source and its direct dependencies.
    defs: std::collections::HashMap<DefinitionId, DefinitionInfo>,
}

struct DefinitionInfo {
    source: DefinitionSource,
    /// Other identifiers referenced by this definition or context.
    deps: Vec<DefinitionId>,
}

enum DefinitionSource {
    /// e.g., `const x = expr`
    VariableDecl(swc_common::Span, Box<swc_ecma_ast::Expr>),

    /// e.g., `import { x } from './y'`
    Import(swc_common::Span),

    /// e.g., `names.forEach(x => ...)` - x is defined by callback parameter binding.
    CallbackParam {
        param_span: swc_common::Span,
        parent_call_span: swc_common::Span,
    },
}
```

Notes:
- The map value includes both the definition source span and precomputed dependency IDs (`deps`) so Phase 2 doesn’t need to rediscover dependencies.
- In v1, if a name is defined multiple times, we will keep the first or last definition deterministically (see “Edge Cases”).

### 2) CallSite output extension

Add a new field:

```rust
pub struct CallSite {
    // ... existing fields ...

    /// Sanitized snippet of code containing all variable definitions relevant to this call,
    /// sorted by line number.
    pub context_slice: Option<String>,
}
```

---

## Phase 1: Indexing Pass (DefinitionIndexBuilder)

### Goal
Walk the entire file once and build a symbol table mapping identifiers to their definition sites.

### Visitor
Create an `IndexingVisitor` using `swc_ecma_visit` that fills a `DefinitionIndex`.

### Helper functions (internal)
1. **Collect bound IDs from patterns**
   - Recursively extract bound identifiers from `Pat`:
     - `Pat::Ident`
     - `Pat::Object`, `Pat::Array`
     - `Pat::Assign` (binds LHS; also contains default value expression)
     - `Pat::Rest`

2. **Collect used IDs from expressions**
   - Walk an `Expr` and collect `Ident` uses.
   - Treat member property names as properties, not variable uses (do chase computed props).

3. **Collect used IDs from default expressions inside patterns**
   - For destructuring with defaults (`{ x = DEFAULT }`), include identifiers used in defaults.

All IDs are computed via `ident.to_id()` from the resolved AST.

### Step-by-step logic

#### 1) Visit `ImportDecl`
- For each import specifier:
  - Bind the local name’s `DefinitionId` to `DefinitionSource::Import(import.span)`
  - `deps = []`
- This establishes a strict boundary: imported symbols are leaf nodes for slicing.

#### 2) Visit `VarDeclarator`
- For each declarator:
  - Extract all bound IDs from `decl.name` (supports destructuring).
  - If `decl.init` exists:
    - `deps = used_ids(init) + used_ids(pattern_defaults)`
    - For each bound ID:
      - store `DefinitionSource::VariableDecl(var_decl.span, init.clone())`
      - store `deps`

#### 3) Visit `CallExpr` (callback parameters)
- For each call argument that is an arrow/function expression:
  - For each callback parameter pattern:
    - Extract bound IDs.
    - Map each bound ID to:
      - `DefinitionSource::CallbackParam { param_span, parent_call_span: call.span }`
      - `deps = used_ids_from_call_context(call)`

`used_ids_from_call_context(call)` collects identifiers from:
- the call’s callee (e.g., `names` in `names.forEach(...)`)
- all non-function arguments

It intentionally does not descend into callback bodies.

---

## Phase 2: Static Slicing (Option B: Anchored Slice)

### Objective
Implement the `CallSiteExtractor` logic to generate the `context_slice` string.

### Inputs
- `call_span`: Span of the API call expression (anchor).
- `seed_expr`: The first argument of the API call (e.g., `arg1` in `app.get(arg1, ...)`).
- `definition_index`: The `DefinitionIndex` built in Phase 1.
- `source_map`: SWC `SourceMap` for converting spans to code strings.

### Algorithm (Recursive Graph Walk / Worklist)

#### Initialize
- `worklist`: queue of `(DefinitionId, depth)` seeded from the identifiers used in `seed_expr`.
- `collected_spans`: set initialized with:
  - `call_span` (the anchor)
  - `seed_expr.span()` (optional; only if it is not fully contained by `call_span`)
- `visited_ids`: set of `DefinitionId` to prevent cycles.

#### Traverse
Repeat until `worklist` is empty or limits are reached:
1. Pop `(id, depth)`.
2. If `id` is in `visited_ids`, continue.
3. If `depth > 20`, stop expanding this branch.
4. Look up `DefinitionInfo` for `id` in `definition_index`.
   - If missing, stop expanding this branch.
5. Action based on the definition source:
   - **VariableDecl**: add the defining statement span to `collected_spans`, then enqueue `deps`.
   - **CallbackParam**: add `parent_call_span` to `collected_spans`, then enqueue `deps` (from the parent call context).
   - **Import**: add the import statement span to `collected_spans` and stop recursion (leaf node).

#### Limits
- Stop expansion if `depth > 20`.
- Stop expansion if `collected_spans.len() > 50`.

### Output Generation (The Slice)
1. Sort `collected_spans` by line number (source order).
2. Merge overlapping spans.
3. Convert each span to a string via `source_map.span_to_snippet()`.
4. Join snippets with `\n` to produce the final `context_slice`.

### Verification
Ensure the output always includes:
- the API call span (the anchor), and
- the definitions/imports needed to explain the identifiers used in that call.

---

## Output Formatting (`context_slice`)

### Requirements
- Always includes the API call anchor span.
- Sanitized source snippets.
- Sorted by line number (source order).
- Deduplicated and overlap-merged.

---

## Edge Case Strategy

### Destructuring assignments
Example: `const { id } = config`
- Treat as a definition of `id`.
- Store the entire `VarDecl` span.
- Dependencies include `config` and any defaults.

### Template literals and concatenation
Example: `` `${base}/users/${id}` ``
- The slice will include definitions for `base` and `id` if resolvable.

### Callback parameter shadowing
Handled by Phase 0 resolver:
- callback param `i` and global `i` have distinct `SyntaxContext`s.

### Missing definitions
If an identifier is not in the index:
- stop recursion for that branch
- still emit whatever was resolvable

### Multiple writes / reassignments
In v1, `DefinitionIndex` maps an ID to a single `DefinitionInfo`.
- If needed, extend to store multiple definition sites and choose based on nearest preceding position.

---

## Integration Plan

### Where it runs
Within the existing Stage 1 call-site extraction pipeline:

Per file:
1. Parse → `Module`
2. Resolver pass (Phase 0)
3. IndexingVisitor builds `DefinitionIndex` (Phase 1)
4. CallSiteExtractor extracts call sites and computes `context_slice` from `DefinitionIndex` (Phase 2)

### Downstream behavior
- Triage remains lean (no `context_slice` in the triage payload).
- Specialist agents receive full call sites and can use `context_slice` when arguments are identifiers/complex.

---

## Test Plan

Add a focused test module to validate both resolver correctness and slicing behavior.

Minimum cases:
1. Resolver canary: same symbol name in different scopes resolves to correct definition.
2. Identifier route: `const route = "/x"; app.get(route, ...)` includes route decl.
3. Imported boundary: `import { route } ...; app.get(route, ...)` includes import and stops.
4. Callback param: `routes.forEach(r => app.get(r, ...))` includes parent call context and routes definition.
5. Destructuring: `const { route } = cfg; app.get(route, ...)` includes destructuring and cfg definition/import.
6. Cycle safety: `const a = b; const b = a; app.get(a, ...)` terminates with bounded slice.

---

## Implementation Checklist (high level)

1. Add resolver dependency and apply resolver to every parsed module.
2. Implement `DefinitionIndex` and `IndexingVisitor`.
3. Implement slicing resolver and span-to-slice formatting.
4. Extend `CallSite` with `context_slice` and wire it into extraction.
5. Add tests verifying resolver correctness and slice quality.
