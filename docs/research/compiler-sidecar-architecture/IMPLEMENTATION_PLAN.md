# Compiler Sidecar Implementation Plan

**Status:** Ready for Implementation  
**Prerequisites:** Read `ARCHITECTURE.md` first

## Overview

This document provides sequential prompts for implementing the Compiler Sidecar architecture. Each prompt is designed to be self-contained and build upon the previous work.

### Key Design Principles

1. **Parallel Startup** - Sidecar spawns immediately at CLI start; TypeScript initialization happens in background while SWC/LLM work proceeds
2. **Framework/Library Agnostic** - Works with any TypeScript HTTP framework (Express, Fastify, Hono, tRPC, etc.)
3. **Implicit Type Inference** - Extract types even when developers don't write explicit annotations
4. **CI-First** - Fast, deterministic execution for CI pipelines

---

## Phase 1: Node.js Sidecar Setup

### Prompt 1.1: Initialize Sidecar Project

> Create a new directory `src/sidecar` in the carrick project. Initialize a Node.js project there with the following:
> 
> 1. `package.json` with:
>    - Name: `@carrick/type-sidecar`
>    - Type: `module`
>    - Main: `dist/index.js`
>    - Scripts: `build` (tsc), `start` (node dist/index.js)
>    - Dependencies: `ts-morph@^25.0.1`, `dts-bundle-generator@^9.0.0`, `zod@^3.23.0`
>    - DevDependencies: `typescript@^5.8.0`, `@types/node@^20.0.0`
> 
> 2. `tsconfig.json` with:
>    - Target: ES2022
>    - Module: NodeNext
>    - ModuleResolution: NodeNext
>    - OutDir: `./dist`
>    - RootDir: `./src`
>    - Strict: true
>    - Declaration: true
> 
> 3. Create the `src/` directory structure with empty placeholder files:
>    - `src/index.ts`
>    - `src/types.ts`
>    - `src/validators.ts`
>    - `src/project-loader.ts`
>    - `src/type-inferrer.ts` (NEW: for implicit type inference)
>    - `src/bundler.ts`

**Acceptance Criteria:**
- [ ] Can run `npm install` without errors
- [ ] Can run `npm run build` (may have empty file errors, that's OK)
- [ ] Directory structure matches specification

---

### Prompt 1.2: Define Message Types and Validators

> In `src/sidecar/src/types.ts`, define the TypeScript interfaces for the sidecar message protocol:
> 
> 1. `SidecarRequest` - union type for 'init', 'bundle', 'infer', 'health', and 'shutdown' actions
> 2. `SymbolRequest` - symbol name, source file, optional alias
> 3. `InferRequest` - file_path, line_number, infer_kind, optional alias (NEW: for implicit types)
> 4. `InferKind` - enum: 'function_return', 'expression', 'call_result', 'variable', 'response_body'
> 5. `SidecarResponse` - request_id, status, dts_content, inferred_types, errors
> 6. `InferredType` - alias, type_string, is_explicit, source_location
> 7. `SymbolFailure` - for reporting individual symbol resolution failures
> 
> In `src/sidecar/src/validators.ts`, create Zod schemas to validate incoming JSON:
> 
> 1. `SymbolRequestSchema`
> 2. `InferRequestSchema` (NEW)
> 3. `SidecarRequestSchema` - discriminated union on 'action' field
> 
> Export a `parseRequest(json: unknown)` function that validates and returns typed request or throws.

**Acceptance Criteria:**
- [ ] Types compile without errors
- [ ] Zod schemas match TypeScript interfaces
- [ ] `parseRequest()` returns correct discriminated union type
- [ ] InferKind enum covers all inference strategies

---

### Prompt 1.3: Implement Project Loader

> In `src/sidecar/src/project-loader.ts`, create a `ProjectLoader` class that:
> 
> 1. Takes a `repo_root` and optional `tsconfig_path` in its constructor
> 2. Initializes a ts-morph `Project` instance
> 3. Handles the case where tsconfig.json doesn't exist (create default compiler options)
> 4. Provides a `getProject()` method to access the loaded project
> 5. Provides a `isInitialized()` method
> 6. Logs any initialization errors to stderr
> 
> Handle common edge cases:
> - Missing tsconfig.json
> - Invalid tsconfig.json
> - Path resolution issues

**Acceptance Criteria:**
- [ ] Can load a ts-morph project from a valid tsconfig
- [ ] Handles missing tsconfig gracefully
- [ ] Logs meaningful error messages

---

### Prompt 1.4: Implement Type Bundler (Physical File Strategy)

> In `src/sidecar/src/bundler.ts`, create a `TypeBundler` class that:
> 
> 1. Takes a ts-morph `Project` and `repoRoot: string` in its constructor
> 2. Has a method `bundle(symbols: SymbolRequest[]): BundleResult`
> 3. Generates a "virtual entrypoint" string with export statements for each symbol
> 
> **CRITICAL: Write to a PHYSICAL FILE, not in-memory!**
> 
> 4. Write the virtual entry to `{repoRoot}/.carrick_virtual_entry.ts`
>    - dts-bundle-generator REQUIRES a physical file to resolve relative imports
>    - Relative paths like `./types/user` must resolve from the repo root
>    - An in-memory file has no "parent directory" and imports will fail
> 
> 5. Add the file to ts-morph project via `project.addSourceFileAtPath()`
> 6. Use `dts-bundle-generator` to bundle the types
> 7. Clean up: remove from project AND delete physical file (use try/finally)
> 8. Returns the bundled `.d.ts` content or error information
> 
> Handle edge cases:
> - Symbol not found in source file
> - Circular type dependencies
> - External/node_modules types
> - **Always clean up the physical file, even on error**
> 
> The virtual entrypoint should look like:
> ```typescript
> // Generated virtual entrypoint - written to {repoRoot}/.carrick_virtual_entry.ts
> export type { User } from './types/user';        // Resolves correctly!
> export type { Order as OrderResponse } from './types/order';
> ```

**Acceptance Criteria:**
- [ ] Can generate correct virtual entrypoint strings
- [ ] **PHYSICAL FILE: Writes to `{repoRoot}/.carrick_virtual_entry.ts`**
- [ ] **RELATIVE IMPORTS: `./types/user` resolves correctly from repo root**
- [ ] Successfully bundles simple types
- [ ] Reports errors for missing symbols
- [ ] **CLEANUP: Deletes physical file on success AND failure (try/finally)**

---

### Prompt 1.5: Implement Type Inferrer (Scope-Based, Framework-Agnostic)

> In `src/sidecar/src/type-inferrer.ts`, create a `TypeInferrer` class that can extract types even when not explicitly annotated:
> 
> 1. Takes a ts-morph `Project` in its constructor
> 2. Has a method `infer(requests: InferRequest[]): InferredType[]`
> 3. Implements framework-agnostic inference strategies:
>    - `inferFunctionReturn(file, line)` - Get return type of function at line
>    - `inferResponseBody(file, line)` - Find .json()/.send()/ctx.body and get argument type
>    - `inferCallResult(file, line)` - Get return type of call expression (for fetch/axios)
>    - `inferVariable(file, line)` - Get type of variable declaration
> 
> **CRITICAL: Use SCOPE-BASED search, NOT line windows!**
> 
> 4. Each method should:
>    - Find the CONTAINING FUNCTION for the target line (not a ±N line window)
>    - Search the ENTIRE function body for terminal statements
>    - This handles large handlers with middleware, validation, logging before response
>    - Report whether type was explicit or inferred (`is_explicit` field)
>    - Handle Promise unwrapping (Promise<T> → T)
>    - Return union type if multiple response types exist (e.g., success vs error)
> 
> 5. Implement `findContainingFunction(sourceFile, targetLine)`:
>    - Find all functions (arrow, declaration, method, expression) in file
>    - Return the innermost function whose range contains the target line
>    - This ensures we search the right scope even for nested functions
> 
> **Framework-agnostic patterns to detect (within function scope):**
> - `res.json(data)` / `res.send(data)` - Express/Fastify style
> - `ctx.body = data` - Koa style
> - `return data` / `return Response.json(data)` - Hono/Web API style
> - Generic `.json()` method calls on response objects

**Acceptance Criteria:**
- [ ] Can infer return type of arrow functions and regular functions
- [ ] Can find res.json()/res.send() and infer argument type
- [ ] Can find ctx.body assignments and infer value type
- [ ] Correctly unwraps Promise<T> to T
- [ ] Reports is_explicit=false for inferred types
- [ ] Works with Express, Fastify, Koa, Hono patterns (framework-agnostic)
- [ ] **SCOPE-BASED: Works for handlers with 50+ lines of setup before response**
- [ ] **UNION TYPES: Returns `User | { error: string }` for handlers with multiple responses**

---

### Prompt 1.6: Implement Main Entry Point and Message Loop

> In `src/sidecar/src/index.ts`, implement the main entry point:
> 
> 1. Create a readline interface listening on stdin
> 2. For each line, parse as JSON and validate with `parseRequest()`
> 3. Handle each action type:
>    - `init`: Create ProjectLoader, store in module-level variable, respond with 'ready' status
>    - `health`: Report initialization status (ready/not_ready) and init_time_ms
>    - `bundle`: Use TypeBundler to bundle explicit types
>    - `infer`: Use TypeInferrer to infer implicit types (NEW)
>    - `shutdown`: Exit process gracefully
> 4. Write JSON response to stdout followed by newline
> 5. Handle errors gracefully - always respond with valid JSON
> 
> Important considerations:
> - Process should stay alive between requests (warm standby)
> - Each request must include `request_id` which is echoed in response
> - stderr is for logging, stdout is only for JSON responses
> - `init` response should include `init_time_ms` for performance monitoring
> - Log `[sidecar] Process started` to stderr immediately on startup

**Acceptance Criteria:**
- [ ] Process stays alive waiting for input
- [ ] Responds with valid JSON to valid requests
- [ ] Responds with error JSON to invalid requests
- [ ] `init` must be called before `bundle`/`infer` works
- [ ] `init` response includes status='ready' and init_time_ms
- [ ] `health` can be called to check readiness
- [ ] Can run: `echo '{"action":"init","request_id":"1","repo_root":"."}' | node dist/index.js`

---

### Prompt 1.7: Add Integration Tests for Sidecar

> Create `src/sidecar/test/` directory with integration tests:
> 
> 1. `test/fixtures/sample-repo/` - A minimal TypeScript project with:
>    - `tsconfig.json`
>    - `src/types.ts` containing `interface User { id: string; name: string; }`
>    - `src/models.ts` containing `interface Order { userId: string; items: string[]; }`
>    - `src/routes.ts` containing handlers WITH and WITHOUT explicit types (for inference testing)
> 
> 2. `test/fixtures/sample-repo/src/routes.ts` should include:
>    ```typescript
>    // Explicit type - for bundle testing
>    export const getUser = (req: Request, res: Response<User>) => { ... };
>    
>    // Implicit type - for inference testing (NO explicit annotation)
>    export const getOrders = async (req, res) => {
>      const orders = await db.findOrders();  // TypeScript knows this is Order[]
>      res.json(orders);
>    };
>    ```
> 
> 3. `test/sidecar.test.ts` - Tests that:
>    - Spawn the sidecar process
>    - Send init request, verify 'ready' status and init_time_ms
>    - Test `bundle`: Request `User` type, verify flattened output
>    - Test `infer`: Request inference at getOrders line, verify type is inferred
>    - Test `health`: Verify readiness reporting
>    - Send shutdown request
> 
> Use Node.js built-in test runner (`node --test`) or add vitest as a dev dependency.

**Acceptance Criteria:**
- [ ] Tests can be run with `npm test`
- [ ] All tests pass
- [ ] Bundle tests verify explicit types are extracted
- [ ] Infer tests verify implicit types are inferred (even without annotations)
- [ ] Tests verify is_explicit=true for explicit, is_explicit=false for inferred

---

## Phase 2: Rust Integration

### Prompt 2.1: Create TypeSidecar Rust Module (Non-Blocking Spawn)

> Create `src/services/type_sidecar.rs` in the Carrick Rust project:
> 
> 1. Define structs matching TypeScript types:
>    - `SymbolRequest` (symbol_name, source_file, alias)
>    - `InferRequest` (file_path, line_number, infer_kind, alias)
>    - `InferKind` enum (FunctionReturn, Expression, CallResult, Variable, ResponseBody)
>    - `SidecarResponse` (request_id, status, initialized, init_time_ms, dts_content, inferred_types, etc.)
>    - `InferredType` (alias, type_string, is_explicit, source_location)
> 
> 2. Define `SidecarState` enum: `Spawning`, `Initializing`, `Ready`, `Failed(String)`
> 
> 3. Create `TypeSidecar` struct with:
>    - `child: Child`
>    - `stdin: Mutex<ChildStdin>`
>    - `stdout: Mutex<BufReader<ChildStdout>>`
>    - `state: Arc<Mutex<SidecarState>>`
>    - `spawn_time: Instant`
> 
> 4. Implement methods:
>    - `TypeSidecar::spawn(sidecar_path: &Path)` - spawns process IMMEDIATELY (non-blocking)
>    - `TypeSidecar::start_init(repo_root, tsconfig)` - sends init, returns immediately
>    - `TypeSidecar::is_ready()` - non-blocking check
>    - `TypeSidecar::wait_ready(timeout: Duration)` - blocking wait with timeout
>    - `TypeSidecar::resolve_types(symbols)` - bundle explicit types
>    - `TypeSidecar::infer_types(requests)` - infer implicit types (NEW)
>    - `TypeSidecar::resolve_all_types(explicit, infer)` - combined method
> 
> 5. Implement `Drop` trait to shutdown the process
> 
> Add to `src/services/mod.rs` and `src/lib.rs`.

**Acceptance Criteria:**
- [ ] Compiles without errors
- [ ] `spawn()` returns immediately (non-blocking)
- [ ] `start_init()` sends init and returns immediately
- [ ] `wait_ready()` blocks until ready or timeout
- [ ] Can send/receive JSON messages
- [ ] Process is killed on Drop

---

### Prompt 2.2: Add Sidecar to CLI Startup (Parallel Initialization)

> Update `src/main.rs` to spawn the sidecar IMMEDIATELY at CLI start:
> 
> ```rust
> fn main() {
>     // STEP 1: Spawn sidecar FIRST (non-blocking)
>     let sidecar = if args.enable_type_extraction {
>         let sidecar = TypeSidecar::spawn(&sidecar_path)?;
>         sidecar.start_init(&repo_path, tsconfig.as_deref());
>         eprintln!("[main] Sidecar spawned, initializing in background...");
>         Some(sidecar)
>     } else { None };
>     
>     // STEP 2: SWC Scanning (PARALLEL with sidecar init)
>     let candidates = swc_scanner.find_candidates(&repo_path)?;
>     
>     // STEP 3: LLM Analysis (PARALLEL with sidecar init)  
>     let results = orchestrator.analyze_files(&candidates).await?;
>     
>     // STEP 4: Wait for sidecar (should already be ready)
>     if let Some(ref sidecar) = sidecar {
>         sidecar.wait_ready(Duration::from_secs(30))?;
>         // ... resolve types
>     }
> }
> ```
> 
> Add CLI flag `--sidecar-type-extraction` (default: false for now).
> Configure sidecar path via `CARRICK_SIDECAR_PATH` env var.

**Acceptance Criteria:**
- [ ] Sidecar spawns BEFORE SWC scanning starts
- [ ] SWC scanning and LLM analysis proceed in parallel with sidecar init
- [ ] `wait_ready()` called AFTER LLM analysis completes
- [ ] Total wall-clock time is MAX(sidecar_init, swc+llm), not SUM
- [ ] Clean shutdown on Ctrl+C

---

### Prompt 2.3: Update LLM Schema for Type Symbols

> Update `src/agents/schemas.rs` to add new fields to the endpoint and data_call schemas:
> 
> For endpoints:
> ```json
> "primary_type_symbol": {
>     "type": "STRING",
>     "nullable": true,
>     "description": "The primary type symbol name without wrappers (e.g., 'User' from 'Response<User[]>')"
> },
> "type_import_source": {
>     "type": "STRING",
>     "nullable": true,
>     "description": "Import path where the type is defined (e.g., './types/user'), null if inline"
> }
> ```
> 
> Update `src/agents/file_analyzer_agent.rs`:
> 1. Add `primary_type_symbol` and `type_import_source` fields to `EndpointResult`
> 2. Add same fields to `DataCallResult`
> 3. Update the system prompt to instruct the LLM to extract these fields
> 
> The prompt should explain:
> - `primary_type_symbol` is just the identifier (User, Order), not the full type (Response<User[]>)
> - `type_import_source` comes from import statements at the top of the file
> - If the type is defined inline or in the same file, `type_import_source` is null

**Acceptance Criteria:**
- [ ] Schema includes new fields
- [ ] EndpointResult and DataCallResult have new fields
- [ ] LLM prompt explains how to extract these fields

---

### Prompt 2.4: Integrate Sidecar into FileOrchestrator (Explicit + Inferred)

> Update `src/agents/file_orchestrator.rs` to use the TypeSidecar:
> 
> 1. Add a method `collect_type_requests(&self, results) -> (Vec<SymbolRequest>, Vec<InferRequest>)`
>    - Iterate through all endpoints and data_calls
>    - For entries WITH `response_type_string`: create `SymbolRequest` (explicit)
>    - For entries WITHOUT `response_type_string`: create `InferRequest` (implicit)
>    - For implicit types, use multiple inference strategies:
>      - `InferKind::ResponseBody` - find res.json()/res.send()
>      - `InferKind::FunctionReturn` - infer handler return type
>    - Generate aliases for each
> 
> 2. Add a method `resolve_types_with_sidecar(&self, sidecar, explicit, infer) -> Result<String, String>`
>    - Call `sidecar.resolve_all_types(explicit, infer)`
>    - Return the bundled .d.ts content (explicit + inferred combined)
> 
> 3. Update `analyze_files` to optionally accept a sidecar reference
>    - If sidecar is provided, call the new type resolution methods
>    - Store bundled types in the result
> 
> Remove or deprecate `enrich_type_positions` - it's no longer needed.

**Acceptance Criteria:**
- [x] Collects explicit type requests (with annotation) - `FileOrchestrator::collect_type_requests()` in `file_orchestrator.rs`
- [x] Collects infer requests (without annotation) - Same method handles both explicit and inferred
- [x] Calls sidecar.resolve_all_types() with both - `FileOrchestrator::resolve_types_with_sidecar()` calls this
- [x] Output .d.ts includes both explicit and inferred types - `TypeResolutionResult.dts_content` populated in `engine/mod.rs`
- [x] Inferred types are marked with `// Inferred` comment in output - Handled by sidecar's bundler
- [x] Old position-based enrichment removed - `enrich_type_positions` never existed in codebase

---

### Prompt 2.5: Update S3 Upload for Bundled Types

> Update `src/cloud_storage/` to handle the new bundled types format:
> 
> 1. In `CloudRepoData` or equivalent, add a field for the bundled `.d.ts` content
> 2. Update the upload logic to store the bundled types as `types.d.ts` (or similar)
> 3. Update the download logic to retrieve bundled types
> 4. Ensure the type manifest (endpoint-to-type mapping) is also uploaded
> 
> The uploaded artifact should include:
> - `types.d.ts` - The bundled type definitions
> - `type-manifest.json` - Mapping of endpoints to their type aliases
> 
> This allows the type checker to load types for multiple repos and compare them.

**Acceptance Criteria:**
- [x] Bundled types are uploaded to S3 - `CloudRepoData.bundled_types` serialized with repo data
- [x] Type manifest is uploaded alongside - `CloudRepoData.type_manifest` serialized with repo data
- [x] Download retrieves both files - `download_all_repo_data` returns full `CloudRepoData` including these fields
- [x] Format is compatible with type checker - `TypeManifestEntry` struct matches expected format

---

## Phase 3: Type Checker Refactor

### Prompt 3.1: Create Manifest-Based Type Matcher

> Create a new file `ts_check/lib/manifest-matcher.ts`:
> 
> 1. Define `TypeManifest` interface:
>    ```typescript
>    interface TypeManifest {
>      repo_name: string;
>      commit_hash: string;
>      entries: ManifestEntry[];
>    }
>    interface ManifestEntry {
>      method: string;
>      path: string;
>      type_alias: string;
>      role: 'producer' | 'consumer';
>      file_path: string;
>      line_number: number;
>    }
>    ```
> 
> 2. Create `ManifestMatcher` class:
>    - `loadManifest(jsonPath: string): TypeManifest`
>    - `findProducersForEndpoint(method: string, path: string): ManifestEntry[]`
>    - `findConsumersForEndpoint(method: string, path: string): ManifestEntry[]`
>    - `matchEndpoints(producers: TypeManifest, consumers: TypeManifest): MatchResult[]`
> 
> 3. Path matching should normalize:
>    - Remove trailing slashes
>    - Convert path params (`:id`, `{id}`, `[id]`) to a canonical form
>    - Case-insensitive comparison

**Acceptance Criteria:**
- [ ] Can load and parse manifest files
- [ ] Path normalization handles common patterns
- [ ] Matching returns correct pairs

---

### Prompt 3.2: Simplify Type Checker

> Update `ts_check/lib/type-checker.ts` to use manifest-based matching:
> 
> 1. Add a new method `checkCompatibilityWithManifests(producerManifest: TypeManifest, consumerManifest: TypeManifest, typesProject: Project)`
> 
> 2. This method should:
>    - Use ManifestMatcher to find endpoint matches
>    - Load the corresponding type aliases from the bundled .d.ts files
>    - Use ts-morph's type assignability checking
>    - Return the same `TypeCheckResult` format
> 
> 3. Deprecate but don't remove the old `checkCompatibility` method (for migration period)
> 
> 4. Update `run-type-checking.ts` to support both modes:
>    - Legacy mode: Parse alias names to match
>    - Manifest mode: Use manifest files to match

**Acceptance Criteria:**
- [ ] Manifest-based checking works
- [ ] Legacy mode still works
- [ ] Same result format from both modes
- [ ] Clear logging indicates which mode is active

---

### Prompt 3.3: End-to-End Test with Sidecar

> Create an end-to-end integration test that:
> 
> 1. Uses the `express-demo-1` test repos (repo-a and repo-b/repo-c)
> 2. Runs the full analysis with sidecar enabled
> 3. Verifies:
>    - Sidecar produces bundled types
>    - Type manifest is generated correctly
>    - Type checking identifies correct matches/mismatches
> 
> The test should compare results between legacy and sidecar modes to ensure parity.
> 
> Create `tests/sidecar_integration_test.rs` with:
> - `test_sidecar_produces_bundled_types`
> - `test_sidecar_handles_missing_types`
> - `test_type_checking_with_sidecar_types`

**Acceptance Criteria:**
- [ ] Tests run with `cargo test`
- [ ] Tests use real TypeScript files
- [ ] Results match expected type relationships
- [ ] CI can run these tests (Node.js available)

---

## Phase 4: Cleanup and Documentation

### Prompt 4.1: Remove Legacy Type Position Code

> Once the sidecar is proven stable, remove the legacy code:
> 
> 1. In `src/swc_scanner.rs`:
>    - Remove `TypePositionFinder` struct
>    - Remove `find_type_position_at_line` function
>    - Remove `find_type_position_at_line_from_content` function
>    - Keep `CandidateVisitor` and `SwcScanner` (still used for AST gating)
> 
> 2. In `src/agents/file_orchestrator.rs`:
>    - Remove `enrich_type_positions` method
> 
> 3. In `ts_check/lib/`:
>    - Remove or archive: `type-extractor.ts`, `type-processor.ts`, `declaration-collector.ts`
>    - Simplify `output-generator.ts` if still needed
>    - Keep: `type-checker.ts`, `manifest-matcher.ts`

**Acceptance Criteria:**
- [ ] Code compiles without legacy type position code
- [ ] All tests pass
- [ ] No dead code warnings

---

### Prompt 4.2: Update Documentation

> Update project documentation:
> 
> 1. Update `.thoughts/OUTSTANDING_WORK.md`:
>    - Mark type extraction issues as resolved
>    - Add any new outstanding work discovered
> 
> 2. Update `.thoughts/project_state_2025.md`:
>    - Document the sidecar architecture
>    - Update the Type Checking System section
> 
> 3. Update `docs/research/file-centric-analysis-architecture.md`:
>    - Add section on type extraction via sidecar
>    - Remove references to position-based extraction
> 
> 4. Create `src/sidecar/README.md`:
>    - How to build the sidecar
>    - How it's used by Carrick
>    - Message protocol reference

**Acceptance Criteria:**
- [ ] All docs are accurate and up-to-date
- [ ] New developer can understand the architecture
- [ ] No references to removed code

---

## Rollback Plan

If the sidecar approach encounters unforeseen issues:

1. **Feature Flag**: The `--sidecar-type-extraction` flag allows instant rollback
2. **Parallel Operation**: Both old and new code can coexist during migration
3. **S3 Format**: If S3 format changes, version the schema

## Success Metrics

- **Accuracy (Explicit)**: Type extraction succeeds for >95% of endpoints with type annotations
- **Accuracy (Inferred)**: Type inference succeeds for >70% of endpoints without annotations
- **Performance**: Sidecar init completes within SWC+LLM time (no added latency)
- **Performance**: Sidecar warm response time <100ms per batch
- **Reliability**: No crashes or hangs in the sidecar process
- **Framework Coverage**: Works with Express, Fastify, Koa, Hono, tRPC (framework-agnostic)
- **CI Time**: No increase in total CI pipeline time (parallel startup)

## Timeline Estimate

| Phase | Duration | Dependencies |
|-------|----------|--------------|
| Phase 1 (Sidecar) | 3-4 days | None |
| Phase 2 (Rust Integration) | 2-3 days | Phase 1 |
| Phase 3 (Type Checker) | 2-3 days | Phase 2 |
| Phase 4 (Cleanup) | 1-2 days | Phase 3 stable |
| **Total** | **8-12 days** | |

## Notes for Implementation

1. **Node.js Version**: Ensure CI has Node.js 18+ for ESM module support
2. **dts-bundle-generator**: May need configuration for certain edge cases
3. **Type Checking**: Consider streaming results for large repos
4. **Error Messages**: Make sidecar errors actionable for users
5. **Parallel Startup**: The key optimization - spawn sidecar FIRST, before any other work
6. **Framework Agnostic**: Never hardcode Express/Fastify/etc patterns - use generic method detection
7. **Implicit Types**: This is a major differentiator - extract types even when developers don't annotate
8. **CI First**: Design for headless execution, deterministic output, fast failure
9. **Scope-Based Search**: NEVER use fixed line windows (±15) - always find containing function and search entire body
10. **Physical Virtual Entry**: Write `.carrick_virtual_entry.ts` to repo root - dts-bundle-generator needs real file for relative imports
11. **Gitignore**: Add `.carrick_*` pattern to `.gitignore` to prevent accidental commits of temp files