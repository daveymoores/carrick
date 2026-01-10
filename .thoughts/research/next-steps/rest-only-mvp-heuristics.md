# REST-only MVP: remaining heuristics + path to framework/library agnosticism

This document focuses on everything *except* cross-repo type checking (covered separately in `cross-repo-type-checking.md`). The goal here is: market Carrick as **REST-only**, while making it **agnostic** to the server framework, client fetching pattern, and API-definition library.

---

## 1) What “REST-only but agnostic” means (practical definition)

**REST-only** (MVP scope):
- Identify API interactions as `(method, normalized path)`
- Optionally associate request body / response body types

**Agnostic** (product promise):
- Works regardless of whether the code uses Express/Fastify/Koa/etc.
- Works regardless of whether the client uses fetch/axios/ky/got/custom wrapper.
- Works regardless of whether types come from handwritten interfaces, zod schemas, OpenAPI-generated types, etc.

**Acceptable built-in heuristics for REST-only**:
- HTTP verbs (GET/POST/PUT/PATCH/DELETE)
- Path normalization and parameter canonicalization

Everything else should be either:
- structural/static (AST-based, not name-based),
- user-configured (e.g. `carrick.json`), or
- LLM-driven interpretation (but fed by robust context such as `context_slice`).

---

## 2) Where heuristics are present today (with code pointers)

### A) Fetch-specific consumer extraction (hard-coded)
In `src/call_site_extractor.rs`:
- `is_fetch_call()` checks `callee ident == "fetch"`
- `extract_fetch_url()` knows fetch argument shapes
- `extract_fetch_method()` parses `options.method` and defaults to `GET`

Impact:
- Consumer extraction is not truly library-agnostic.
- Custom clients and other libraries won’t be normalized equivalently.

### B) `.json()` correlation (fetch-response specific)
In `src/call_site_extractor.rs`:
- `correlated_fetch` is attached only when `callee_property == "json"` and the receiver variable was assigned from `fetch()`.

Impact:
- Assumes the “decode step” is `.json()` on a fetch `Response`.
- Does not generalize to `axiosResponse.data`, `resp.text()`, `resp.body`, schema decoders, etc.

### C) Prompt-level bias toward specific libraries
In `src/gemini_service.rs`, the system message explicitly frames extraction as:
- “Extract ONLY HTTP requests (fetch, axios, request libraries)”

Impact:
- Even if the intent is broad, naming specific libraries is a soft heuristic that can bias extraction.

### D) Mock-mode heuristics baked into tests
In `src/gemini_service.rs`:
- `generate_mock_triage_response()` classifies call sites using heuristics on `callee_property`:
  - HTTP verbs (`get/post/put/delete/patch`) → `HttpEndpoint`
  - `use` / `register` → mount vs middleware heuristics
  - `json` / `urlencoded` → `Middleware`
- There is also a Koa-specific hack for fixtures.

Impact:
- This is *only* in mock mode, but tests encode framework assumptions.

### E) URL normalization policy (mostly acceptable)
In `src/url_normalizer.rs`:
- normalizes env var patterns (`ENV_VAR:` and `process.env.`)
- normalizes template literals (`${}` → `:param`)
- strips host/query/fragment, normalizes slashes

Impact:
- This is largely a necessary REST matching policy, not a framework heuristic.
- However, normalization rules should be treated as *policy* (configurable), because small differences can change matching behavior.

### F) Type-checking wrapper heuristic (`Response<T>`)
In `ts_check/lib/type-checker.ts`:
- `unwrapResponseType()` unconditionally tries to unwrap `Response<T>` for producers.

Impact:
- Couples type checking to one wrapper type.
- Breaks “agnostic” for clients/servers that use different response envelopes.

### G) Structural limitation: member-call receiver must be an `Ident`
In `src/call_site_extractor.rs`, `visit_call_expr()` only records member calls where:
- the receiver is `Expr::Ident` and
- the property is `MemberProp::Ident`

Impact:
- Misses many realistic patterns that are common in both server and client code:
  - chaining (`axios.create(...).get(...)`)
  - factory clients (`client().get(...)`)
  - nested namespaces (`api.client.http.get(...)`)

This is a coverage limitation that directly harms “agnostic” behavior.

---

## 3) How to get to agnostic behavior without exploding complexity

### 1) Treat “HTTP call detection” as classification, not hard-coded detection
Hard-coding `fetch()` detection does not scale.

Better model:
- Capture *candidate* call sites structurally.
- Run triage/classification (LLM or rules) to decide:
  - is this an HTTP call?
  - is it an HTTP endpoint definition?
  - is it a router mount/middleware?

Key enabler already added:
- `context_slice` on call sites provides single-file definitions/imports so classification can be accurate without brittle pattern matching.

A practical guardrail:
- Keep a cheap structural prefilter (e.g. only calls with URL-like first args or calls within async/await contexts), then delegate semantics to triage.

### 2) Make library-specific behaviors pluggable/configurable
Even if the core is agnostic, adapters can improve precision:
- fetch adapter, axios adapter, etc.

But keep them:
- opt-in
- driven by config (`carrick.json`) or LLM “framework guidance” output

### 3) Generalize “decode step” correlation beyond `.json()`
Instead of special-casing `.json()`, treat “decode” as:
- an operation that consumes a value produced by a call classified as HTTP.

This can be generalized using the same single-file use–def infrastructure:
- relate `resp` to `resp.json()`
- relate `response` to `response.data`
- relate `result` to `parse(result)`

### 4) Fix call capture so it covers real-world patterns
To be meaningfully agnostic, Carrick must record call sites where the callee is not a simple `Ident` receiver.

This is primarily a structural extraction improvement (not a heuristic). Once the call is captured, you can rely on `context_slice` + triage to interpret it.

---

## 4) Should Carrick analyze request/response bodies (object literals)?

### Where it helps
When explicit types are missing, analyzing object literals at boundaries can catch obvious mismatches:
- missing required fields
- wrong field names
- incompatible primitive types

This is especially useful for:
- request bodies passed inline
- responses constructed inline

### Where it hurts
Bodies are often:
- built from variables/spreads
- validated/transformed by runtime schema libraries
- partial and completed server-side

Pure “shape diffing” can create false positives.

### Recommendation
- Do **not** replace type checking with body-shape analysis.
- Use body-shape analysis as a *supplemental, low-confidence* signal when type info is missing.
- Make it opt-in and clearly reported as heuristic.

---

## 5) Next steps (prioritized)

1) Reduce/encapsulate fetch + `.json()` assumptions
- Move consumer extraction toward `context_slice` + triage-driven semantics.
- Keep only verb + path normalization as built-in REST matching semantics.

2) Improve call-site capture coverage
- Record chained/factory/namespaced callees so agnostic classification is possible.

3) Align type checking with agnostic goals
- Replace `Response<T>` unwrap with a configurable wrapper policy.
- Stop pairing endpoints via alias naming (use manifest-driven keys).

4) Add optional boundary body-shape analysis
- Only as a fallback when types are unavailable.

---

## Appendix: code references
- Fetch URL/method extraction: `src/call_site_extractor.rs` (`is_fetch_call`, `extract_fetch_url`, `extract_fetch_method`)
- `.json` correlation: `src/call_site_extractor.rs` (`correlated_fetch` in `visit_call_expr`)
- Gemini system prompt: `src/gemini_service.rs` (`create_extraction_system_message`)
- Mock triage heuristics: `src/gemini_service.rs` (`generate_mock_triage_response`)
- URL normalization policy: `src/url_normalizer.rs`
- `Response<T>` unwrap: `ts_check/lib/type-checker.ts` (`unwrapResponseType`)
