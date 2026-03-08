# Outstanding work

This is the single "what's left to do" document for Carrick.

For deeper analysis and rationale, see:
- `research/next-steps/rest-only-mvp-heuristics.md`
- `research/next-steps/cross-repo-type-checking.md`
- `docs/research/compiler-sidecar-architecture/` - Implemented solution for type extraction

---

## Recently completed (relevant to the current direction)

- **Compiler Sidecar Architecture (Phase 1-4)** ✅
  - Node.js sidecar (`src/sidecar/`) with ts-morph + dts-bundle-generator
  - Rust `TypeSidecar` struct for process management
  - LLM schema updated for `primary_type_symbol` and `type_import_source`
  - Sidecar integrated into FileOrchestrator
  - Manifest-based type matching (`ts_check/lib/manifest-matcher.ts`)
  - Legacy position-based code archived to `ts_check/lib/_legacy/`
  - Type inference support for implicit types
- Single-file static slicing for call sites (`context_slice`).
- Agent prompts updated to actively use `context_slice` during extraction.
- Library/framework-agnostic call correlation via `correlated_call` (no `fetch()`/`.json()`-specific data model).
- Mock-mode responses no longer use fixture/framework name checks; mounts/endpoints can be inferred using structural cues and `context_slice` (e.g. extracting `prefix: '/api/v1'`).

---

## Highest-priority outstanding work

### 1) Make REST-only matching truly library/framework agnostic

Goal: the core should not rely on fetch/axios/Express-specific assumptions beyond HTTP verbs + path normalization.

Work:
- [x] Reduce hard-coded `fetch()` and `.json()` assumptions in extraction and correlation logic.
- Improve call-site capture coverage so it includes chained/factory/namespaced callees (not just `ident.method()`).
- Treat "is this an HTTP call / endpoint / mount / middleware" as classification, powered by `context_slice`, rather than pattern matching.
- Fix noisy/incorrect env-var configuration suggestions.
  - Cause: `Analyzer::analyze_matches_with_mount_graph()` (`src/analyzer/mod.rs`) uses `is_env_var_base_url()` to detect env-var base URLs in multiple formats (e.g. `ENV_VAR:NAME:/path`, `${process.env.NAME}/path`, `process.env.NAME + "/path"`). But `Config::is_internal_call()` / `Config::is_external_call()` (`src/config.rs`) only recognize the canonical `ENV_VAR:` prefix (plus domain prefixes). So even *configured* env vars in `${process.env.NAME}` / `process.env.NAME + ...` form fall through to the "Unclassified env var" suggestion path.
  - Solution:
    - Canonicalize env-var routes before classification and suggestion generation: convert `${process.env.NAME}/path` and `process.env.NAME + "/path"` into `ENV_VAR:NAME:<normalized_path>`.
    - Use `UrlNormalizer` to compute `<normalized_path>` (strip query strings, normalize template params) so suggestions don't include noisy variants like `?userId=${...}`.
    - Deduplicate/group suggestions (e.g. group by `(env_var, method, normalized_path)` and show a count + a few sample locations), rather than one suggestion per call site.

### 2) Cross-repo type checking enhancements

**Status:** Core architecture implemented ✅

The compiler sidecar architecture has replaced the error-prone position-based approach:
- ✅ Node.js sidecar with ts-morph + dts-bundle-generator
- ✅ Symbol-based type extraction (not position-based)
- ✅ Type inference for implicit types via TypeScript's inference engine
- ✅ Flat `.d.ts` bundle generation with all dependencies
- ✅ Manifest-based endpoint matching

Remaining work:
- [ ] Configurable wrapper/envelope unwrapping (remove hard-coded `Response<T>` assumption)
- [ ] Expand the contract to check request + response types per endpoint
- [ ] Migrate `src/analyzer/mod.rs::extract_types_for_repo()` to use sidecar instead of legacy script

### 3) Test and fixture hardening

Work:
- [x] Reduce framework-specific heuristics embedded in mock-mode responses used by tests.
- Add fixtures that exercise non-trivial call shapes (chaining, wrappers, schema decoders) and validate coverage improvements.

---

## Deprioritized / optional

- Internal refactors that don't change output (e.g. further simplification of legacy visitors) should be queued behind correctness/coverage work.
- Complete removal of legacy type extraction code (currently archived in `ts_check/lib/_legacy/`) - defer until all callers migrated to sidecar.