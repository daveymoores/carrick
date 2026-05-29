# Function-Signature Types

## Goal

When an agent asks the MCP layer for a function (via `list_function_intents` or
`search_by_intent`), the result should carry the function's **typed signature**
so the agent can call it confidently without a second round trip or a source
fetch. This is the TypeScript-community selling point: every intent the agent
gets back is typed.

Sibling design to [per-endpoint-resolved-types.md](./per-endpoint-resolved-types.md).
Function signatures reuse that pipeline rather than inventing a parallel one.

## The core insight: provenance is metadata, not a routing decision

Two **independent** axes describe a slot's type:

- **Provenance** — was it annotated (`is_explicit: true`) or inferred by the
  compiler (`false`)?
- **Shape/depth** — is it a *named* type (`AuthResult`) or an *anonymous
  structural* type (`{ userId: string; roles: Role[] }`), and how deep?

They do not correlate: ts-morph can infer a clean named type
(`Promise<AuthResult>` — implicit + named), and a human can annotate an ugly
deep inline type (explicit + structural). Therefore **serving strategy keys on
shape/depth, never on provenance.** `is_explicit` rides along only as a
confidence flag the agent reads.

This corrects an earlier wrong idea ("inline implicit types, drill-down explicit
types"). A deep inferred type could pull in half a library's `.d.ts`, so it must
never be inlined — same as a deep explicit type. Both follow the same
hint-and-link pattern.

## Serving model (locked)

Two tiers, identical for every type regardless of provenance:

1. **Hint (inline, bounded).** A compact one-line signature, composed at CI time
   by the scanner, surfaced inline in the intent tools. It uses ts-morph's
   *default* `getText()` form, which keeps named types as names and auto-truncates
   deep structures (`AxiosResponse<…>`). The hint is bounded by the compiler
   itself — it is never the full expansion.
2. **Link (drill-down).** `get_type_definition(service, alias)` returns the full
   resolved + transitively-expanded form. Opt-in, per type the agent actually
   cares about. **No change to this tool is needed — it already resolves by
   alias.**

### MCP output shape (target)

```json
{
  "name": "verifyToken",
  "intent": "Validates the caller's JWT and returns the resolved user...",
  "signature": "(token: string, opts?: VerifyOpts) => Promise<AuthResult>",
  "types_explicit": { "token": true, "opts": true, "return": false },
  "file_path": "src/auth.ts",
  "line_number": 42,
  "hint": "get_type_definition(service, 'AuthResult') to expand a named type"
}
```

`signature` is the composed hint. `types_explicit` is built from the per-slot
`is_explicit` flags. Both `list_function_intents` and `search_by_intent` carry
these fields.

## Scope of THIS work (Phases 1+2 — explicit + inferred hints)

The first change ships the **hint** for both explicit and inferred signatures,
plus the `is_explicit` provenance. It does **not** yet bundle signature named
types for drill-down — that is follow-up issue 1.

In scope:

- Capture explicit param/return types (already captured — see "What exists").
- Infer the gaps via the sidecar (un-annotated params and returns), marked
  `is_explicit: false`.
- Compose the one-line `signature` hint at CI time.
- Surface `signature` + `types_explicit` in both intent tools.

Out of scope (tracked as follow-up issues, see end):

- Bundling signature **named** types into `bundled_types` so
  `get_type_definition` can drill into them.
- Discovering named-type references *inside* inferred type strings.

## What exists today (verified)

- **Explicit signature types are already captured and uploaded.**
  `FunctionDefinition.return_type: Option<String>` (visitor.rs:117) and
  `FunctionArgument.type_string: Option<String>` (visitor.rs:18-22) hold raw TS
  annotation text. These serialize into `function_definitions` inside
  `CloudRepoData` and are already stored in DynamoDB. The MCP layer currently
  **discards** them.
- **The MCP type model drops them.** `lambdas/mcp-server/src/types.ts`:
  `FunctionArgument` is `{ name }` only (line 162-164); `FunctionDefinition` has
  no `return_type` (line 139-160).
- **The sidecar can already infer a function's return type.**
  `type-inferrer.ts:188 inferFunctionReturn` resolves the containing function,
  reads `getReturnTypeNode()` to set `isExplicit`, and uses
  `returnType.getText(func)` for the string. There is **no** param-inference path.
  NOTE: `inferFunctionReturn` applies Promise/wrapper unwrapping for
  endpoint-payload purposes. **Signature inference must NOT unwrap** — a function
  that returns `Promise<AuthResult>` should show exactly that. Use a distinct
  path/flag that skips `unwrapTypeWithConfig` / `unwrapPromise`.
- **Type-request collection is endpoint-only.**
  `file_orchestrator.rs:272 collect_type_requests` walks endpoints/data-calls
  only (`endpoint_lookup`, `data_call_lookup`, `should_infer_request_body`).
  There is no function-signature collection pass. This is the main new code.
- **The intent `calls` field is unrelated.** `FunctionDefinition.calls` is built
  by substring matching (`intent_generator.rs:53 body.contains(fn_name)`) and is
  not part of this work. (See issues #55/#58.)

## Data model changes

### Scanner (`carrick`)

`FunctionArgument` (visitor.rs:13-23):

```rust
pub struct FunctionArgument {
    pub name: String,
    #[serde(skip)]
    pub type_ann: Option<TsTypeAnn>,
    /// Explicit annotation text OR inferred type text (filled by the
    /// signature pass). `None` only if both annotation and inference failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_string: Option<String>,
    /// true = from a source annotation; false = compiler-inferred.
    #[serde(default)]
    pub is_explicit: bool,
}
```

`FunctionDefinition` (visitor.rs:92-118) gains:

```rust
/// true = return type from a source annotation; false = inferred.
#[serde(default)]
pub return_is_explicit: bool,
/// One-line signature hint composed at CI time, e.g.
/// "(token: string, opts?: VerifyOpts) => Promise<AuthResult>".
#[serde(default, skip_serializing_if = "Option::is_none")]
pub signature: Option<String>,
```

`return_type` already exists. After the signature pass, `return_type` holds the
explicit annotation if present, else the inferred type; `return_is_explicit`
records which.

Composing `signature` at CI time keeps the MCP a pure serving layer (the same
philosophy as per-endpoint-resolved-types.md). Best-effort optional markers
(`?`) come from param optionality; if not readily available, omit them in the
first cut — the types are the important part.

### MCP (`carrick-cloud`)

`lambdas/mcp-server/src/types.ts`:

```ts
export interface FunctionArgument {
  name: string;
  type_string?: string;
  is_explicit?: boolean;
}
export interface FunctionDefinition {
  // ...existing...
  return_type?: string;
  return_is_explicit?: boolean;
  signature?: string;
}
```

## Implementation

### 1. Scanner: signature-collection pass (`carrick`)

Add a pass (alongside `collect_type_requests` in `file_orchestrator.rs`, sharing
its sidecar `infer` batch) that, for each entry in `function_definitions`:

- For each parameter: if `type_string` is `Some` → `is_explicit = true`. Else
  emit an `InferRequestItem` (new `InferKind::FunctionParam`) to infer it;
  `is_explicit = false`.
- For the return: if `return_type` is `Some` → `return_is_explicit = true`.
  Else emit an `InferRequestItem` with `InferKind::FunctionReturn` (skipping the
  wrapper/Promise unwrapping — see sidecar note below); `return_is_explicit =
  false`.
- After the batched `infer` call returns, merge inferred strings back onto the
  `FunctionDefinition` and compose `signature`.

Decide which functions to include: at minimum every exported function with a
captured definition. Closures captured as `*_handler` (visitor.rs:600-665) are
in `function_definitions` too; including them is fine and gives typed handler
signatures, but expect implicit-any params for genuinely untyped standalone
functions (still an honest signal — emit `any`, `is_explicit: false`).

`InferRequestItem` (type_sidecar.rs:105) needs a way to identify which parameter
a `FunctionParam` request targets — add `param_name: Option<String>` (one
request per param, resolved by name within the function located by line/span).

### 2. Sidecar: param inference (`carrick/src/sidecar`)

- Add `FunctionParam` to `InferKind` (Rust `type_sidecar.rs:49`, TS
  `type-inferrer.ts` + `validators.ts` + `types.ts`).
- Add `inferFunctionParams` in `type-inferrer.ts`, modeled on
  `inferFunctionReturn` (line 188): resolve the containing function via
  `resolveContainingFunction`, then for the named parameter use
  `param.getTypeNode()` (explicit) vs `param.getType().getText(param)`
  (inferred/contextual). Set `is_explicit` from whether `getTypeNode()` exists.
- For signature **return** inference, do NOT reuse the unwrapping branch of
  `inferFunctionReturn`. Either add a flag to skip
  `unwrapTypeWithConfig`/`unwrapPromise`, or a thin `inferSignatureReturn` that
  returns `func.getReturnType().getText(func)` with `isExplicit` from
  `getReturnTypeNode()`.

### 3. MCP serving (`carrick-cloud`)

- `lambdas/mcp-server/src/types.ts` — add the fields above.
- `lambdas/mcp-server/src/tools/find-similar.ts` (`list_function_intents`,
  line ~41-49) — add `signature` and `types_explicit` to each
  `FunctionIntentEntry`.
- `lambdas/check-or-upload/index.js` — the `search-by-intent` action projection
  (lines 708-714) builds `{ name, file_path, line_number, intent, similarity }`.
  Add `signature: def.signature` and `types_explicit` (from
  `def.arguments[].is_explicit` + `def.return_is_explicit`). Then widen
  `SearchByIntentResult` in `mcp-server/src/types.ts` and pass through in
  `tools/search-by-intent.ts`.

No `get_type_definition` change. No `get_endpoint_types` change.

## Cross-repo coordination

- `carrick` work on branch `feat/function-signature-types` (this branch).
- `carrick-cloud` work needs its own parallel `feat/function-signature-types`
  branch. NOTE: `carrick-cloud` is currently on `feat/workspaces-projects`
  (an unrelated feature) — branch from `main`/the appropriate base, not from it.
- The new `FunctionDefinition` fields serialize automatically into the uploaded
  `CloudRepoData`; no upload-format coordination needed beyond shipping the
  scanner first (the MCP fields are optional, so an older store is handled).

## Non-goals

- No backwards-compat shims (Carrick has no users) — ship the new shape, delete
  any old shape in the same commit.
- No drill-down of signature named types yet (follow-up issue 1).
- No changes to the `calls`/`called_intents` substring matching (issues #55/#58).

## Follow-up issues (filed separately)

1. **Make function-signature named types resolvable via `get_type_definition`.**
   Extend the signature pass to emit `SymbolRequest`s for explicit named
   param/return types, widening the symbol-set sent to the sidecar `bundle`
   action. dts-bundle-generator pulls transitive deps automatically; reuse the
   `DefinitionResolver` so each gets `resolved_definition` + `expanded_definition`,
   served via the existing `get_type_definition`.

2. **Discover named-type references inside inferred signature types.** When the
   sidecar infers a slot that references a named type (e.g. inferred return
   `Promise<AuthResult>`), parse/walk out the named references and feed them into
   the symbol-set so the inferred named sub-type gets the same drill-down as an
   explicit one. Depends on issue 1.
</content>
