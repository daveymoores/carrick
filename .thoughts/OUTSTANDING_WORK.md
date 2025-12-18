# Outstanding work

This is the single “what’s left to do” document for Carrick.

For deeper analysis and rationale, see:
- `research/next-steps/rest-only-mvp-heuristics.md`
- `research/next-steps/cross-repo-type-checking.md`

---

## Recently completed (relevant to the current direction)

- Single-file static slicing for call sites (`context_slice`).
- Agent prompts updated to actively use `context_slice` during extraction.

---

## Highest-priority outstanding work

### 1) Make REST-only matching truly library/framework agnostic

Goal: the core should not rely on fetch/axios/Express-specific assumptions beyond HTTP verbs + path normalization.

Work:
- Reduce hard-coded `fetch()` and `.json()` assumptions in extraction and correlation logic.
- Improve call-site capture coverage so it includes chained/factory/namespaced callees (not just `ident.method()`).
- Treat “is this an HTTP call / endpoint / mount / middleware” as classification, powered by `context_slice`, rather than pattern matching.

### 2) Make cross-repo type checking robust and deterministic

Goal: type checking should not depend on alias naming conventions or a single wrapper type.

Work:
- Manifest-driven endpoint identity (pair producers/consumers using a stable endpoint key from Rust, not derived from alias strings).
- Configurable wrapper/envelope unwrapping (remove the hard-coded `Response<T>` assumption).
- Support inferred/implicit types by emitting inferred aliases when explicit annotations are missing.
- Expand the contract to check request + response types per endpoint (then iterate to params/query/status variants).

### 3) Test and fixture hardening

Work:
- Reduce framework-specific heuristics embedded in mock-mode responses used by tests.
- Add fixtures that exercise non-trivial call shapes (chaining, wrappers, schema decoders) and validate coverage improvements.

---

## Deprioritized / optional

- Internal refactors that don’t change output (e.g. further simplification of legacy visitors) should be queued behind correctness/coverage work.
