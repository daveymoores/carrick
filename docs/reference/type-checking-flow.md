# Type Checking Flow (Manifest-Based)

This document explains how cross-repo type checking works, the inputs it uses,
and where mismatches or missing matches can be introduced.

## Overview

Carrick builds a producer/consumer manifest from multi-agent analysis, resolves
types via the sidecar, and then runs a manifest matcher + TS compiler checks to
verify compatibility. Matching is **method + normalized path + type kind**.
Call-site suffixes (`_Call<hash>`) are only for uniqueness and do not affect
matching.

## Inputs

- Source files from each repo.
- `carrick.json` for internal/external env vars and domains.
- `package.json` for dependency reporting.

## End-to-End Flow

1) File discovery + imports
   - Parse files and collect imported symbols for framework detection.
   - These imports are used later to reconcile LLM type import sources.

2) AST-gated file analysis
   - SWC gatekeeper finds candidate lines.
   - SWC also gathers call-chain context (callee text, method literal when present, URL/path literal/template, enclosing function name, argument shapes, return variable names, nearby imports) so the agent can classify without hardcoded heuristics.
   - LLM extracts mounts/endpoints/data_calls from those targets using the provided context bundle instead of baked-in fetch/axios/`.json()` rules.
   - If the initial LLM output is suspicious, a retry is attempted.
   - We now keep the richer result if a retry drops findings.

3) Mount graph construction
   - All file results are combined.
   - Mount paths are resolved to build full endpoint paths.
   - Path joining normalizes to avoid `//` double slashes.

4) Manifest entries
   - Producer entries: one request + one response per resolved endpoint.
   - Consumer entries: one request + one response per data_call.
   - Consumer aliases include `_Call<hash>` for unique call sites.
   - Matching ignores alias names; only method/path/type_kind matter.

5) Type sidecar resolution
   - Explicit types: bundle symbols with `type_import_source`.
   - Inline types: stored as inline aliases (`Response<{...}>`).
   - Inferred types: `infer_types` for call results or handler bodies.
   - If bundling fails, we still append `unknown` aliases so type checking can run.

6) Bundled types + manifest files
   - Bundled types from sidecar are stored in S3 (or mock).
   - `ts_check/output/*_types.d.ts` is recreated for all repos.
   - `producer-manifest.json` and `consumer-manifest.json` are written.

7) Matching + type checking
   - `ts_check/lib/manifest-matcher.ts` normalizes paths and methods.
   - Normalization handles params (`:id`, `{id}`, `[id]`) and numeric segments.
   - Producer/consumer pairs with the same normalized method + path + kind match.
   - TS compiler checks `producer` assignable to `consumer` for each match.

## Why Matches Sometimes Disappear

- LLM retry returns fewer data_calls; consumers vanish.
  - Fixed by keeping the richer result.
- Path mismatch (`/users/1` vs `/users/:id`).
  - Fixed by normalizing numeric segments to `:param`.
- Double slashes in mounted paths (`//users`).
  - Fixed by normalizing mount joins.

## Why Types Become `unknown`

`unknown` is used when:
- Bundling fails (invalid import source, missing dependency).
- A type alias is declared in the manifest but no type was resolved.

This leads to errors like:
- Producer: `unknown`
- Consumer: `Response<...>`

## Why a Manifest Alias Is Not Resolved by the Sidecar

The manifest is generated for **all** endpoints and calls, but the sidecar only
resolves aliases that it can explicitly bundle or infer. A manifest alias can
remain unresolved when:

- The LLM provides a `type_import_source` that is not actually imported in the file.
- The LLM supplies an inline type but also sets a `primary_type_symbol`, so we
  skip inline aliasing and still fail to infer.
- Inference fails because the line does not contain a suitable call expression
  (e.g., wrapper functions, non-call expressions, or mismatched line numbers).
- Bundling fails due to missing dependencies or an invalid module path.

When this happens, we still emit the alias in the manifest, but the sidecar
does not produce a corresponding `export type` in `*_types.d.ts`, so it is
filled with `unknown` during alias padding.

## Current Pain Points Seen In Logs

- LLM returns incorrect `type_import_source` (e.g., `express` or `react`).
  - We now reconcile those with actual imports and drop invalid ones.
- LLM sometimes sets `primary_type_symbol` to inline shapes.
  - Inline types should use `response_type_string` and skip bundling.
- Data calls created for `.json()` parsing with malformed `method`.
  - New approach: supply the full call-chain context to the agent (upstream call, path/method literal, parsing chain, enclosing function) and have the agent classify whether a downstream consumer exists. Avoid heuristic suppression or fetch/axios hardcoding; rely on the structured context.

## How to Debug a Run

1) Check manifests:
   - `ts_check/output/producer-manifest.json`
   - `ts_check/output/consumer-manifest.json`
2) Check bundled types:
   - `ts_check/output/repo-a_types.d.ts` (and repo-b/repo-c)
3) Validate that matched producer/consumer paths normalize the same way.
4) Confirm the producer alias exists in the bundled types:
   - If missing, it will be `unknown`.

## Key Invariants

- Matching is based on normalized method + path + type_kind only.
- Call-site suffixes only prevent alias collisions.
- Type checking is only as strong as the resolved aliases in `*_types.d.ts`.

## Planned Direction (Agnostic + Context-First)

- Feed the agent the exact import table, AST span, and call-chain facts so it can decide if a parsing chain is an HTTP consumer. No framework- or client-specific heuristics.
- Tighten the agent schema to require explicit classification (consumer vs. not-a-consumer) with rationale tied to the provided context bundle.
- Keep inline payloads authoritative: when `response_type_string` is present, skip symbol bundling; if a symbol is requested, validate it against imports/locals and fall back to inference when it fails.
- Use the sidecar/compiler to derive inline aliases from real type nodes (no LLM guesses) and to infer when symbol lookup is invalid.
- Record stable AST coordinates (file, start, span, callee text) from SWC so the sidecar can walk to the true call expression before asking the checker for types.
