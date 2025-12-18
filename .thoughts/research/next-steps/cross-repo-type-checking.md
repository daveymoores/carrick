# Cross-repo type checking (next steps)

This document focuses on Carrick’s **cross-repo TypeScript type checking**: what exists today, where it breaks relative to “plug in and play”, and what needs to change to make it robust.

Related background:
- Existing system overview: `../ts_check.md`

---

## 1) Current approach (what exists today)

Carrick currently does cross-repo type compatibility checking via the `ts_check/` pipeline:

1. **Rust analysis** identifies endpoints (producers) and calls (consumers), and collects type information when it can (primarily via explicit type annotations).
2. **Type extraction** (`ts_check/extract-type-definitions.ts`) uses ts-morph to locate referenced types in the source repo and emits a **staged, minimal** TypeScript project containing the extracted declarations plus transitive dependencies.
3. **Type checking** (`ts_check/run-type-checking.ts` + `ts_check/lib/type-checker.ts`) loads staged files and checks assignability.

Key behaviors in the checker today:
- Endpoint pairing relies on **alias naming conventions** (`parseTypeName()` in `ts_check/lib/type-checker.ts`).
- Producers are unwrapped via a **hard-coded `Response<T>`** assumption (`unwrapResponseType()` in `ts_check/lib/type-checker.ts`).
- Compatibility is checked using TypeScript’s assignability rules (`Type.isAssignableTo()`) with diagnostics.

### What’s good about this
- Uses the real TypeScript type system for compatibility and diagnostics.
- Staged project is fast relative to compiling whole repos.
- Scales with the number of extracted types rather than total repo size.

---

## 2) Where it breaks against “plug in and play”

### A) Endpoint identity is derived from alias naming (brittle)
The checker currently derives endpoints from type names. This is brittle because:
- Renames or collisions break matching.
- Aliases must be deterministic across repos.
- “Endpoint conversion” logic becomes part of correctness.

### B) Wrapper semantics are hard-coded (`Response<T>`)
`unwrapResponseType()` assumes producer types look like `Response<T>`. This breaks for:
- `AxiosResponse<T>`
- `Promise<T>`
- custom envelopes (`ApiResponse<T>`, `{ data: T }`, etc.)

### C) Explicit-type bias; inferred/implicit types are under-supported
The current extraction flow is position-based and largely depends on explicit type annotations being present. In real codebases, key types are frequently inferred:
- `const data = await resp.json()` (no annotation)
- `return res.json(value)` where `value` is inferred

If no explicit type is written, “extract definition by offset” has nothing stable to attach to.

### D) Staged project fidelity differs from real project fidelity
The staged project is fast, but it can resolve types differently from the real repo due to:
- `tsconfig` differences (paths, module resolution)
- ambient types, module augmentation
- dependency resolution nuances

This can cause false positives/negatives.

### E) The contract checked is incomplete
Today the emphasis is “producer response” vs “consumer expected response”. A REST contract typically also includes:
- request body type
- query/params (at least structurally)
- status-code variants

---

## 3) What is required for a robust cross-repo type system

### 1) Manifest-driven endpoint pairing (stop deriving identity from alias names)
Carrick already knows the canonical endpoint (method + normalized path). The checker should not attempt to re-derive it from alias strings.

**Requirement**: a stable endpoint key produced by Rust and carried into type checking.

Pragmatic implementation:
- Emit a manifest JSON alongside staged types, e.g. `type-manifest.json` mapping alias →
  - `endpoint_key` (e.g. `"GET /api/users/:id"`)
  - `direction` (`producer` | `consumer`)
  - `call_id` (for consumers)
  - `source_location` (file/line/col)
- Update `TypeCompatibilityChecker` to group by `endpoint_key` from the manifest.

This immediately removes the biggest source of brittleness.

### 2) Configurable unwrap/envelope policy
Replace `Response<T>` hard-coding with a policy that supports common wrappers.

Examples of unwrap layers:
- `Promise<T>`
- `Response<T>`
- `AxiosResponse<T>`
- custom envelopes like `{ data: T }`

This policy should be data-driven (config or manifest), not hard-coded.

### 3) A strategy for implicit (inferred) types
There are two viable approaches:

#### Option A — Keep staged project; *emit inferred types*
When Carrick needs a type at a boundary but no explicit annotation exists:
- open the repo’s tsconfig in a TS tool (ts-morph / TS compiler API)
- compute `typeAtLocation(node)`
- emit `export type Alias = <inferred type>;` into the staged project

Pros:
- Incremental evolution of `ts_check`.
- Works even when devs didn’t annotate types.

Cons:
- Requires robust mapping from Rust spans/offsets to TS nodes.
- Inferred type text can be huge; needs size/depth caps and canonicalization.

#### Option B — Use full-repo compilation artifacts
Run `tsc --emitDeclarationOnly` (or equivalent) per repo and compare using declarations.

Pros:
- Highest fidelity to real compilation.

Cons:
- Often slow/flaky at org scale; harder CI story.

**Recommendation for MVP**: start with Option A, but keep Option B as an escape hatch.

### 4) Expand to request+response parity
To validate REST contracts meaningfully across repos, the minimum contract should include:
- request body type (if present)
- response body type (if present)

Then expand to params/query/status codes as follow-on phases.

---

## 4) Proposed roadmap

### Phase 1 — Manifest-driven matching
- Emit a manifest with stable endpoint keys.
- Update the checker to pair based on manifest keys.

### Phase 2 — Wrapper policy
- Replace `unwrapResponseType()` with configurable unwrappers.

### Phase 3 — Inferred type emission
- When explicit types are missing, compute and emit inferred aliases.
- Add hard caps and “unknown” fallback behavior.

### Phase 4 — Request + response parity
- Ensure both request and response types are extracted/checked for each endpoint.
- Define compatibility policy (strict vs allowing extra fields, etc.).

---

## 5) Success criteria
- Endpoint pairing is stable under refactors/renames.
- Wrapper semantics are configurable (not `Response<T>`-specific).
- “No explicit annotation” cases still produce types and checks.
- CI output is actionable: mismatches point to specific endpoint keys and source locations.
