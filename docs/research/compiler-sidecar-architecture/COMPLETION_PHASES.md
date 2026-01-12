# Carrick Compiler Sidecar Completion Phases

This document is a handoff plan for finishing the compiler sidecar refactor and
restoring correct producer/consumer type checking across repositories.

It is written for a fresh ChatGPT session. It includes the current state, gaps,
and a phased plan with concrete changes and acceptance criteria.

## Repository Context

Carrick is a framework-agnostic REST analysis tool for TypeScript services. It
extracts:
- Producer endpoints (server handlers)
- Consumer calls (fetch/axios/etc.)
- Then checks request/response type compatibility for matching endpoints.

The new compiler sidecar (`src/sidecar/`) replaces brittle position-based type
extraction with TypeScript compiler-powered bundling and inference. A primary
goal is to extract **implicit** types using the TypeScript compiler; this is a
core requirement, not optional. The Rust CLI spawns the sidecar and orchestrates
LLM analysis + type resolution.

Key folders:
- Rust engine: `src/`
- Type sidecar: `src/sidecar/`
- Type checker: `ts_check/`
- Research docs: `docs/research/compiler-sidecar-architecture/`

Do not run Terraform commands in this repo.

## Current Known Gaps (Do Not Skip)

1) `run_final_type_checking` calls `run-type-checking.ts` without the required
   `--producer` and `--consumer` manifest paths. This always fails.

2) No manifest with method/path/role/type_kind is produced. Sidecar outputs only
   `alias/type_string` and Rust drops endpoint metadata.

3) Upload/download of type files still uses legacy `ts_check/output/*_types.ts`,
   not `cloud_data.bundled_types` from the sidecar.

4) Inferred types are never emitted into a `.d.ts`, so implicit types never
   reach the checker.

5) Bundling assumes file paths only; module specifiers like `express` or
   `%none%` fail validation and cause missing types.

6) There is no consistent canonical URL normalization path. The analyzer uses
   `UrlNormalizer`, but `EndpointAnalysis::urls_could_match` still uses a
   separate heuristic.

7) Request types are not extracted; only response types exist in the LLM schema.

Legacy code paths should be removed, not preserved. This tool is not live, and
backwards compatibility is not required.

## Definitions (Use Consistently)

- Producer: an API endpoint handler (server side).
- Consumer: a call site that invokes an API (client side).
- Request type: the request body shape. For consumers, this is the request
  payload; for producers, this is the handler's `req.body` type or explicit
  `Request<..., ReqBody>` annotation.
- Response type: the response body shape. For consumers, this is the type of
  `response.json()` or equivalent; for producers, it is the type sent via
  `res.json(...)` or explicit `Response<T>` annotation.
- Canonical path: `UrlNormalizer` output (method + normalized path).

The goal is to match producer and consumer entries by method + normalized path,
then check:
- producer.request vs consumer.request
- producer.response vs consumer.response

If a type is unknown/untyped, represent it explicitly (e.g., `unknown`) and
surface it in the type checker output.

Implicit types must be captured via the compiler and included in `.d.ts` output
as first-class entries.

## Phase 0: Align on Manifest Model

### Tasks
- Define a single manifest entry schema that includes:
  - method
  - path
  - role: `producer` | `consumer`
  - type_kind: `request` | `response`
  - type_alias
  - file_path
  - line_number
  - is_explicit
  - type_state: `explicit` | `implicit` | `unknown`
- Update Rust `TypeManifestEntry` and TS `ManifestEntry` to match.

### Acceptance Criteria
- Rust `CloudRepoData.type_manifest` and TS `ManifestEntry` carry the same fields.
- Any entry can be matched to a specific endpoint (method/path) and type kind.

## Phase 1: Generate Producer/Consumer Manifests in Rust

### Tasks
- Build manifests from the mount graph:
  - Producers from `ResolvedEndpoint` (method + full_path).
  - Consumers from `DataFetchingCall` using `UrlNormalizer` for path.
- For each endpoint/call, create two entries (request + response).
- Create stable type aliases (do not rely on name parsing):
  - Example: `Endpoint_<hash>_Request`, `Endpoint_<hash>_Response`
  - Hash can be method + path + role + type_kind.
- Write two manifest files to `ts_check/output/`:
  - `producer-manifest.json`
  - `consumer-manifest.json`

### Acceptance Criteria
- `ts_check/run-type-checking.ts` can be invoked with both manifests and runs.
- For a repo with endpoints and calls, manifests contain entries for both
  request and response.

## Phase 2: Extend LLM Schema to Capture Request Types (Explicit Only)

### Tasks
- Update `FileAnalysisResult` schema to include:
  - `request_type_string`
  - `request_primary_type_symbol`
  - `request_type_import_source`
  - `request_type_position` (optional)
- Update the LLM prompt/schema to extract request types when explicitly
  annotated (do not infer).
- Keep response body inference in the sidecar for implicit types.

### Acceptance Criteria
- When request types are explicitly annotated, they appear in analysis results.
- No inference is attempted for request types in the LLM layer.

## Phase 3: Update Sidecar Type Requests + Bundle Module Specifiers

### Tasks
- Extend `SymbolRequest` to allow module specifiers (e.g., `express`) in
  `source_file`.
- Update `TypeBundler.generateVirtualEntrypoint`:
  - If `source_file` starts with `.` or `/`, treat as a path.
  - Otherwise treat as a module specifier.
- Allow validation to pass for module specifiers:
  - Skip `fs.existsSync` checks for non-path sources.
- Add explicit handling for placeholders like `%none%` or `%inline%`:
  - These should not be sent to the bundler.

### Acceptance Criteria
- `Response`/`Request` from `express` bundle successfully.
- Sidecar no longer emits "Source file not found: express".

## Phase 4: Emit Inferred + Untyped Types into Bundled `.d.ts`

### Tasks
- After explicit bundle, append inferred aliases to the `.d.ts` output:
  - `export type Alias = <type_string>;`
- For unknown/untyped cases, emit:
  - `export type Alias = unknown;`
- Ensure `bundled_types` includes explicit + inferred + unknown aliases.

### Acceptance Criteria
- Inferred types appear in generated `.d.ts` and are available to ts-morph.
- Unknown types are still present and tracked in the manifest.

## Phase 5: Wire Type Checking End-to-End

### Tasks
- Update `run_final_type_checking` to pass `--producer` and `--consumer`.
- Ensure `recreate_type_files_and_check` writes `.d.ts` from
  `cloud_data.bundled_types` instead of legacy `ts_check/output/*_types.ts`.
- Update S3 upload to use `cloud_data.bundled_types` (not `ts_check/output`).

### Acceptance Criteria
- Type checking runs without the "missing manifest" error.
- Cross-repo checking uses sidecar-generated types only.

## Phase 6: Enforce Canonical URL Normalization

### Tasks
- Replace `EndpointAnalysis::urls_could_match` with `UrlNormalizer`.
- Use `UrlNormalizer` for consumer call paths when creating manifest entries.
- Keep TS-side `normalizePath` as a final normalization step, but base all
  matching on `UrlNormalizer` output.

### Acceptance Criteria
- A call to `${SERVICE_URL}/users/${id}` matches `/users/:id`.
- There is exactly one canonical normalization path in Rust.

## Phase 7: Remove Legacy Extraction and Matching Code

### Tasks
- Remove `extract_types_for_current_repo` and any legacy extraction paths.
- Remove alias-based matching that depends on old conventions.
- Use the new manifest + sidecar types exclusively.

### Acceptance Criteria
- No legacy type files are generated at all.
- Only sidecar-based types and manifests are used for type checking.

## Phase 8: Tests

### Tasks
- Add Rust tests for manifest generation (method/path/type_kind/role).
- Add TS tests for `ManifestMatcher` to handle request/response pairs.
- Add sidecar tests for module specifier bundling and inferred type emission.
- Add an integration test that runs `tsc` end-to-end against generated
  `.d.ts` files and manifests to validate producer/consumer compatibility.

### Acceptance Criteria
- `cargo test` passes.
- `cd src/sidecar && npm test` passes.
- `ts_check` tests (if any) pass.
- The integration test executes `tsc` and asserts expected compatibility or
  mismatch results.

## Notes on Type Compatibility

Type compatibility should check:
- Producer response assignable to consumer response.
- Producer request assignable to consumer request.

If either side is `unknown`, mark as untyped rather than "compatible" or
"incompatible". This should show in results so users see missing typing.

## Suggested Execution Order

1) Phase 0 + Phase 1 (manifest schema + generation).
2) Phase 5 (wire manifests into checker so it runs).
3) Phase 3 + Phase 4 (fix missing types + inference emission).
4) Phase 2 (request types).
5) Phase 6 + Phase 7 (canonical normalization + remove legacy).
6) Phase 8 (tests).
