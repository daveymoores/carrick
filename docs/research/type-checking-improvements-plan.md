# Type Checking Improvements: Implementation Plan (Agent-Ready)

Purpose: add deterministic, framework-agnostic improvements to the LLM->sidecar
handoff and compiler inference. This plan is written for a lower-cost model to
implement. Follow it in order.

Rules for the implementing agent:
- Commit after every stage. Do not combine stages in a single commit.
- Use Conventional Commits (examples included per stage).
- Do not amend commits.
- Keep the system framework- and library-agnostic. No hardcoded framework rules.
- Tests, formatting, and build steps must pass after each stage.

---

## Stage 0: Baseline and Schema Alignment

Goal: make the handoff schema explicit and testable before behavior changes.

Work:
- Define a new "evidence" struct in Rust for analysis outputs:
  - file_path, span_start, span_end (byte offsets), line_number
  - infer_kind (string enum)
  - is_explicit, type_state (explicit | implicit | unknown)
- Extend the flat LLM output schema to include span_start/span_end for:
  - endpoint handlers
  - data call expressions
  - response emission expressions (if present)
- Wire the new fields through to the manifest entries.

Files likely touched:
- src/agents/schemas.rs
- src/agents/file_analyzer_agent.rs
- src/agents/file_orchestrator.rs
- src/manifest/*.rs (or wherever TypeManifestEntry is defined)

Acceptance criteria:
- LLM JSON schema validates with the new fields (optional to start).
- Manifest entries include evidence fields with defaults when missing.
- No behavior change yet; only additional fields.

Tests:
- Rust unit tests for schema serialization/validation.
- Run: cargo test (target the schema tests if available), cargo fmt.

Commit:
- Example: feat(sidecar): add evidence fields to analysis schema

---

## Stage 1: Stable AST Targeting (IDs + Spans)

Goal: stop relying on "near line" heuristics; use stable AST locations.

Work:
- Update SWC gatekeeper to emit a stable candidate_id per call site and the
  exact span (start/end byte offsets).
- Include candidate_id + span_start/span_end in the LLM prompt hints.
- Require the LLM to echo candidate_id in its output for each endpoint/data_call.
- Map candidate_id back to the exact node in Rust (no line window).

Files likely touched:
- src/swc_scanner.rs
- src/agents/file_analyzer_agent.rs
- src/agents/file_orchestrator.rs

Acceptance criteria:
- Each LLM result links to a concrete AST node via candidate_id.
- The sidecar inference requests can be created without line windows.

Tests:
- Rust unit tests for SWC scanner candidate spans.
- Update any fixtures used by file analyzer tests.
- Run: cargo test (targeted), cargo fmt.

Commit:
- Example: feat: add stable candidate ids and spans to SWC scan

---

## Stage 2: Symbol Table Validation (No Invalid Type Hints)

Goal: avoid LLM-provided type symbols that do not exist in the file.

Work:
- Build a per-file symbol table:
  - local type aliases, interfaces, imported symbols, and namespace imports.
- When LLM provides a type symbol or import source, accept it only if:
  - symbol exists in the table, and
  - import source matches an actual import.
- Otherwise: drop the symbol and force inference.

Files likely touched:
- src/agents/file_orchestrator.rs
- src/analysis/imports.rs (or similar)
- src/sidecar/type_request_builder.rs (if exists)

Acceptance criteria:
- Invalid type hints no longer reach the sidecar.
- Valid explicit symbols still bundle correctly.

Tests:
- Add a test with an invalid symbol and confirm it gets ignored.
- Run: cargo test (targeted), cargo fmt.

Commit:
- Example: fix: validate type symbols against real imports

---

## Stage 3: LLM Emits Expressions, Not Types

Goal: LLM should emit only "where the type is," not "what the type is."

Work:
- Modify LLM schema to emit:
  - payload_expression_span for request bodies
  - response_expression_span for response bodies
  - call_expression_span for consumer results
- Remove or ignore response_type_string in downstream flow.
- Sidecar requests now use spans to get type from the compiler.

Files likely touched:
- src/agents/schemas.rs
- src/agents/file_analyzer_agent.rs
- src/agents/file_orchestrator.rs
- src/services/type_sidecar.rs (request building)

Acceptance criteria:
- All inference requests are span-based (no line windows).
- Types are derived from ts-morph getTypeAtLocation.

Tests:
- Update schema tests and one integration fixture.
- Run: cargo test (targeted), cargo fmt.

Commit:
- Example: feat: move to span-based type inference requests

---

## Stage 4: Sidecar Span-Based Inference

Goal: the sidecar accepts spans and resolves types at exact nodes.

Work:
- Extend sidecar InferRequest to include span_start/span_end.
- In ts-morph, locate the node by span and infer:
  - call_result
  - response_body (expression)
  - request_body (expression)
- Keep "unknown" if the final type is Response/unknown.

Files likely touched:
- src/sidecar/src/types.ts
- src/sidecar/src/validators.ts
- src/sidecar/src/type-inferrer.ts
- src/sidecar/src/index.ts

Acceptance criteria:
- Inference works even when the target expression is multiline.
- No "near line" windows remain in the sidecar.

Tests:
- Add a sidecar test with a multiline expression.
- Run: npm run build, npm test.

Commit:
- Example: feat(sidecar): infer types by span instead of line windows

---

## Stage 5: Deterministic Def-Use Walk (Decode Chain Generalization)

Goal: decode chains are inferred by real usage, not hardcoded .json() patterns.

Work:
- In the sidecar, when infer_kind=call_result:
  - find the containing function
  - follow the returned/assigned value to the terminal expression
  - ask the compiler for the type of that terminal expression
- Do not hardcode fetch/axios; use actual AST references.

Acceptance criteria:
- For await chain like await foo().bar().baz(), the terminal expression type
  is used.
- No library-specific checks added.

Tests:
- Add a test with chained calls and destructuring.
- Run: npm run build, npm test.

Commit:
- Example: feat(sidecar): add deterministic def-use inference for call results

---

## Stage 6: Optional Wrapper Registry (AST-Verified)

Goal: allow minimal unwrapping of known wrappers while staying deterministic.

Work:
- Create a small registry keyed by detected packages (Rust side) that declares:
  - wrapper type name
  - unwrap rule (property access or generic param)
- In sidecar, unwrap only if:
  - the wrapper type resolves to the exact symbol from that package
  - the AST shows the matching access (e.g., resp.data)
- If not verified, keep unknown.

Acceptance criteria:
- No unwrap occurs without AST verification.
- Disabling the registry yields the same behavior as before.

Tests:
- Add a fixture for a wrapper type and for a false positive.
- Run: cargo test (targeted), npm run build, npm test, cargo fmt.

Commit:
- Example: feat: add AST-verified wrapper unwrapping registry

---

### Stage 6 Status (Completed)

Completed work:
- Added Rust wrapper registry keyed by detected packages: `src/wrapper_registry.rs`.
- Wired wrapper rules through the Rust->sidecar request:
  - `SidecarRequest::Infer` now includes optional `wrappers`.
  - `TypeSidecar::infer_types` + `resolve_all_types` accept wrapper rules.
  - `FileOrchestrator::resolve_types_with_sidecar` now uses `wrapper_rules_for_packages`.
  - `engine` passes `Packages` into the call chain.
- Sidecar protocol updated to accept wrapper rules (`types.ts`, `validators.ts`, `index.ts`).
- Sidecar inference now unwraps only when:
  - The wrapper type resolves to the package symbol (via declaration path or aliased symbol), or
  - The wrapper type is imported from the package in the source file.
- Added sidecar fixtures + tests (including a local type false-positive guard):
  - `src/sidecar/test/fixtures/sample-repo/src/wrapper-usage.ts`
  - `src/sidecar/test/fixtures/sample-repo/src/wrapper-false-positive.ts`
  - `src/sidecar/test/fixtures/sample-repo/node_modules/wrapper-lib/index.d.ts`
- `.gitignore` and `src/sidecar/.gitignore` allow the wrapper-lib fixture path.

Current wrapper registry:
- `axios` → `AxiosResponse` unwrap via `data` property.
- Extend in `src/wrapper_registry.rs` as needed.

Tests run:
- `npm run build` (in `src/sidecar`)
- `npm test` (in `src/sidecar`)
- `cargo fmt`
- `CARRICK_API_ENDPOINT=https://test.example.com cargo test wrapper_registry`

Notes for the next agent:
- `CARRICK_API_ENDPOINT` is required at build time for `cargo test`.
- Wrapper rules are optional; if none are sent, behavior matches pre-Stage 6.

---

## Stage 7: Evidence Trail in Manifests + Reporting

Goal: make mismatches explainable in CI output.

Work:
- Include evidence fields in producer/consumer manifests:
  - file_path, span_start/end, infer_kind, is_explicit, type_state
- Update reporting to surface evidence when types are unknown or incompatible.

Files likely touched:
- src/manifest/*.rs
- ts_check/lib/manifest-matcher.ts
- ts_check/run-type-checking.ts

Acceptance criteria:
- Output includes evidence for each mismatch and unknown type.
- Manifests are stable and match on method + normalized path.

Tests:
- Update manifest snapshot tests (if any).
- Run: cargo test (targeted), npm run build, npm test.

Commit:
- Example: feat: include evidence trail in manifests and reports

---

## Final Verification (after Stage 7)

Run full checks (if feasible in CI for this repo):
- cargo fmt
- cargo test
- npm run build (in src/sidecar)
- npm test (in src/sidecar)

If full tests are too heavy, run the minimum set required by the stage and
document the full suite as a follow-up task.
