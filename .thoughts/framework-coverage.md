# Framework Coverage Audit

How close is Carrick to framework-agnostic REST support across Express, Koa, Fastify, Hono, NestJS, Hapi — and what's the concrete gap list to close it?

**TL;DR**: Architecturally, the LLM-driven pipeline (`FrameworkGuidanceAgent` → `FileAnalyzerAgent`) is genuinely framework-agnostic. The gap is entirely in the **deterministic scaffolding around the LLM**: the SWC candidate scanner, the type-sidecar's response-body heuristic, and a handful of hard-coded framework name lists. NestJS is the only framework with a hard architectural gap (decorator routing is invisible to the candidate scanner). Koa, Hapi need medium-effort fixes. Fastify and Hono should mostly work out-of-the-box but are completely untested.

---

## 0. Hand-off orientation

**If you're picking this up cold, start here.**

**Current state.** This document is audit + plan only; no code changes have shipped from it. The published Carrick works on Express; everything else has the gaps catalogued below. The NestJS decorator gap is verified by a throwaway test (§2.3 notes the result); other gaps are code-inspection-level confidence unless explicitly marked "verified."

**How to read this doc:**
- **§1–§6** are the audit. Findings, gap list, reference material.
- **§7** is the ship list. Tactical sequence for unblocking OSS teardown posts.
- **§9** is direction of travel. Explains *why* the §7 steps look the way they do — read this before implementing §7, or you'll pick the wrong approach.
- **§10** is CI / fixture infrastructure. Build alongside §7 Step 2 at the earliest.

**If you're implementing, do §7 in order.** Step-level acceptance criteria:

- **Step 1** (Koa `ctx.body` + Hapi `h.response` response detection) — done when the `koa-api` and `hapi-*` fixtures produce correct response types end-to-end. Implement as the **payload-expression schema change in §9 Move 1**, not by extending `['json', 'send', 'end', 'write']` at `type-inferrer.ts:469`. If you're reaching for the whitelist, you've picked the wrong fix.
- **Step 2** (wire orphan fixtures into real tests) — done when a Rust test exercises the pipeline against `tests/fixtures/fastify-api/` and `tests/fixtures/koa-api/` and asserts endpoint counts and types. Settle §10.3 (CI shape) before writing the harness so Step 2 and §10 don't conflict.
- **Step 3** (NestJS decorator support) — done when a NestJS controller fixture produces non-zero candidates and correct endpoints. **Implement as §9 Move 2: widen the SWC scanner to emit candidates for all call expressions and decorator calls, then let the LLM filter.** The cost trade-off (more LLM tokens per scan) is accepted for MVP; do not hesitate over it. Do not add framework-specific branches to the scanner.
- **Step 4** (GraphQL detection banner) — done when a repo using `graphql-request` / `@apollo/client` / similar triggers the §4.3 banner in the report. Orthogonal to the other steps.
- **Step 5** — grab bag. Move 1 subsumes most of it; do last.

**Verify before acting on these claims:**
- §2.8 says `src/extractor.rs:163, 260, 301` hardcodes `"res"` / `"req"` and "may be dead code." Confirm whether that module is reachable in the live pipeline (`engine/mod.rs` or `multi_agent_orchestrator.rs`) before either deleting it or extending it. If dead, just delete.
- §2.6 describes a `.tsx` import-resolution edge case. Low frequency — don't fix unless a real repo hits it.
- §6 OSS repo recommendations were verified on 2026-04-17 against live `package.json` files. Re-check before targeting a teardown post; framework choices drift.

**Defaults already decided — do not re-litigate:**
- **Widen the SWC scanner** (§9 Move 2) when NestJS support lands. The LLM-cost trade-off is accepted.
- **Never add framework-specific strings or name lists** to Rust or TypeScript source. If a fix seems to require one, the fix is wrong; re-read §9.
- **No external infrastructure** (rules repos, rule caches outside this repo, community-contribution systems). MVP scope is "ship from this repo only."
- **Hand-off posture**: execute the plan. When the plan has an opinion, follow it without pausing. When you discover a claim in this doc is wrong, correct the doc in the same PR and continue.

**Act without asking:**
- Schema additions to `FileAnalyzerAgent` output (§9 Move 1, Move 3).
- Passing `ImportSymbolExtractor` output into `FileAnalyzerAgent`'s per-file prompt (§9.3 Move 3) — pure prompt enrichment.
- New fixtures under `tests/framework-fixtures/` per §10.
- Deleting `src/extractor.rs` (or parts of it) if §2.8's "possibly dead code" turns out to be confirmed dead. Just do the verification first.
- Edits to this document when findings change.
- Any change that replaces a framework-specific heuristic in source with LLM-generated guidance or prompt enrichment.

**Escalate (open a PR comment or draft PR and wait) only for:**
- Destructive git operations on shared history (force-push to `main`, deleting branches, rewriting published commits).
- Changes that require external credentials, accounts, or infrastructure provisioning.
- Public API or CLI surface changes to Carrick (new flags, breaking output format changes, changes to `action.yml`).
- A finding that invalidates the plan's core premise (e.g., the `FrameworkGuidanceAgent` pipeline doesn't actually work the way §9.1 describes).

**If you discover a required decision the plan doesn't cover:** pick the most MVP-ish option, document the choice in this doc in a new subsection, and continue. Don't block.

**Load-bearing files to know:**
- `src/swc_scanner.rs` — the candidate gate (§2.3)
- `src/agents/file_analyzer_agent.rs` — the per-file LLM call; schema lives here
- `src/agents/framework_guidance_agent.rs` — the per-scan framework rule generator
- `src/sidecar/src/type-inferrer.ts` — response/request body type inference (§2.2)
- `src/services/type_sidecar.rs` — Rust side of the sidecar; hardcoded type list at lines 797–815 (§2.1)

---

## 1. Summary Table

Ratings: **tested** = real test exercises the path · **code-present-untested** = the code would likely handle it but no test/fixture validates · **gap** = a concrete code/prompt change is required.

| Framework | Producer extraction | Consumer extraction | Response-type inference | Param/body-type inference |
|---|---|---|---|---|
| **Express** | tested | tested | tested | tested |
| **Koa**       | code-present-untested *(mount via `router.routes()` is structurally awkward)* | code-present-untested | **gap** *(`ctx.body = x` is an assignment, not a `.json()/.send()` call)* | code-present-untested *(`ctx.request.body`)* |
| **Fastify**   | code-present-untested *(`app.register(plugin, { prefix })` — plugin mount, not variable)* | code-present-untested | code-present-untested *(bare `return value` works via `inferFunctionReturn`; `reply.send()` works via whitelist)* | code-present-untested |
| **Hono**      | code-present-untested *(`.get`/`.post`/`.route` match)* | code-present-untested | code-present-untested *(`c.json()` is in the whitelist)* | code-present-untested *(`await c.req.json()`)* |
| **NestJS**    | **gap** *(decorator routing invisible to SWC candidate scanner)* | code-present-untested | **gap** *(handlers return values; `@nestjs/common` wrapper types not in untyped-response filter)* | **gap** *(`@Body()` parameter decorators not recognized as request-body sources)* |
| **Hapi**      | code-present-untested *(`server.route({ method, path, handler })` — route config is an object literal)* | code-present-untested | **gap** *(`h.response(x)` uses `response` which is not in the `json/send/end/write` whitelist; bare `return x` works via fallback)* | code-present-untested *(`request.payload` not `request.body`)* |

Tested fixtures that actually run in CI:
- `tests/fixtures/scenario-{1,2,3}-*/` — all Express
- `tests/fixtures/imported-routers/` — Express
- `tests/fixtures/fastify-api/server.ts` — **orphan, no Rust test references it** (`grep fastify-api tests/*.rs` returns nothing)
- `tests/fixtures/koa-api/server.ts` — **orphan, no Rust test references it**

---

## 2. Where Express Assumptions Leak

Concrete file:line references — these are the gap list.

### 2.1 `src/services/type_sidecar.rs:797-815` — `is_untyped_response_type`

```rust
matches!(base,
    "Response" | "Request" | "NextFunction" |       // Express
    "Express" | "Application" | "Router" |          // Express
    "Context" | "FastifyInstance" | "FastifyReply" | "FastifyRequest" |  // Fastify + Koa
    "Koa" | "ParameterizedContext" |                // Koa
    "IncomingMessage" | "ServerResponse"            // Node core
)
```
**Missing**: Hono (`Hono`, `Context` is already there but ambiguous), Hapi (`ResponseToolkit`, `ResponseObject`, `Request`), NestJS (`ExecutionContext`, framework-side `Response`). When a handler's return-type resolves to one of these, the inferrer should treat it as "no real payload type" and emit `unknown`; otherwise the raw framework type leaks into the bundled `.d.ts`.

### 2.2 `src/sidecar/src/type-inferrer.ts:469, 486` — response-body method whitelist

```ts
if (['json', 'send', 'end', 'write'].includes(methodName)) {
```
Hardcoded method names for "this is a response emission". Covers:
- Express: `res.json(x)` ✓, `res.send(x)` ✓, `res.end(x)` ✓
- Fastify: `reply.send(x)` ✓ (but Fastify idiomatic is `return x` — falls through to `inferFunctionReturn`)
- Hono: `c.json(x)` ✓ (but `c.text(x)`, `c.html(x)`, `c.body(x)` are **missing**)
- Koa: **broken** — `ctx.body = x` is an *assignment expression*, not a method call; the whole `resolveResponsePayloadNode` path at `type-inferrer.ts:445-498` only looks for `CallExpression` children
- Hapi: **broken** — `h.response(x)` uses method name `response`, which isn't in the list; bare `return x` works via `inferFunctionReturn` fallback
- NestJS: mostly `return x` — works via fallback; but `@Res() res.status(200).json(x)` (Express-underneath escape hatch) uses `.json()` so it works

### 2.3 `src/swc_scanner.rs:220-312` — AST candidate gatekeeper

```rust
fn is_potential_api_object(&self, name: &str) -> bool { /* "app", "router", "server", ... */ }
fn is_potential_api_method(&self, name: &str) -> bool { /* "get", "post", "register", "route", ... */ }
```
These lists are reasonable for call-expression-based frameworks. But the visitor only emits candidates when:
1. `call.callee` is `Expr::Member(member)` (i.e., `obj.method(...)`), OR
2. The callee is a bare `Ident` named literally `fetch` (`swc_scanner.rs:315-322`)

**Critical gap — CONFIRMED by test**: NestJS decorator calls like `@Get('/users')` appear in the AST as `Decorator { expr: CallExpr { callee: Ident("Get"), args: [...] } }`. The visitor's `visit_call_expr` does fire on the inner `CallExpr`, but the callee is `Expr::Ident("Get")`, not `Expr::Member`. The member-call branch doesn't match, and the global-fetch branch only matches the literal `"fetch"`. **Result: zero candidates emitted → NestJS files are short-circuited out of the pipeline at `src/swc_scanner.rs:136 (should_analyze = false)`.**

Verified with a throwaway test (`SwcScanner::scan_content` on a classic `@Controller('users')` / `@Get() / @Get(':id') / @Post()` controller): `should_analyze = false`, `candidates.len() = 0`. Not speculation.

Also missed: Hapi's `server.route({...})` DOES emit a candidate (both `server` and `route` are in the lists), but the first-arg snippet is an `ObjectLit`, not a string — the LLM must extract method/path from inside the object.

### 2.4 `src/wrapper_registry.rs:8-20` — deterministic wrapper unwrapping

```rust
if dependencies.contains_key("axios") {
    rules.push(WrapperRule { package: "axios", type_name: "AxiosResponse", unwrap: Property("data") });
}
```
Only `axios` is wired. Missing wrappers:
- `node-fetch` / Web API `Response` → `.json()` returns `Promise<unknown>`, so there's nothing to unwrap; but the `Response` *type* leaks through `is_untyped_response_type` (see 2.1)
- `@nestjs/common` — `Observable<T>` returns from services; `HttpService` (from `@nestjs/axios`) returns `Observable<AxiosResponse<T>>` — two layers to unwrap
- `got` — `got` returns typed responses with `.body`
- `ky` — similar to `got`
- `@hapi/hapi` — `h.response(x)` returns `ResponseObject`, no useful unwrap

The TypeScript-side `ExtractionConfig` in `src/sidecar/src/type-inferrer.ts:785-797` is more flexible (it takes `wrapperSymbols` + `originModuleGlobs` + `payloadGenericIndex` rules), but no caller actually generates config for these packages.

### 2.5 `src/framework_detector.rs:194, 200` — LLM prompt framework list

```
- Identify all frameworks/libraries used for HTTP routing, e.g., express, koa, fastify, hapi, nestjs.
- Identify all libraries used for data fetching or HTTP clients, e.g., axios, node-fetch, got, superagent, graphql-request.
```
**Hono is not named**. The LLM is capable enough to identify it from dependencies, but the examples anchor the output; explicit inclusion is cheap.

### 2.6 `src/agents/file_orchestrator.rs:983-988` — import resolution defaults to `.ts`

```rust
let with_extension = if !resolved_str.ends_with(".ts") && !ends_with(".tsx") && !ends_with(".js") && !ends_with(".jsx") {
    format!("{}.ts", resolved_str)          // ← always .ts
}
```
An extensionless import (`import X from './routes/users'`) resolves only to `.ts`. If the target file is `./routes/users.tsx`, the canonicalized path won't exist and mount-cross-file-linking silently fails. Realistic for Express-mounted components in Next.js-shaped monorepos. Low-frequency but real. Fix: try `.ts`, `.tsx`, `./index.ts`, `./index.tsx` in order.

### 2.7 `src/engine/mod.rs:337` and `src/file_finder.rs:103` — file walking

```rust
matches!(ext, "ts" | "tsx" | "js" | "jsx")
```
Good — `.tsx` IS walked, for both producer and consumer extraction (they share the same walk). Task brief asked for `.ts` only on the producer side; the current implementation is broader but harmless (Express endpoints aren't defined in `.tsx`, so false positives are rare).

### 2.8 `src/extractor.rs:163, 260, 301` — hardcoded `res` / `req` identifiers

The legacy extractor hardcodes `ident.sym == "res"` and `obj.sym == "req"` when walking function bodies to pull request/response shapes. This is pure Express convention — Koa handlers take `ctx`, Hapi takes `(request, h)`, Hono takes `c`, Fastify uses `(request, reply)`. Any file whose handler uses non-`req`/`res` parameter names gets silently skipped by this extractor.

This module may be partially superseded by the newer file-centric pipeline (`FileAnalyzerAgent`), but it's still compiled in and reachable. Confirm whether it's dead code; if not, replace with either a config-driven list or remove it in favor of the LLM-driven path.

---

## 3. Per-Framework Gap Analysis

### 3.1 Koa

**Producer**: `router.get('/users', handler)` works. The rub is the mount step: `app.use(router.routes())` — the `child` of the mount is a function *call result*, not a variable. The `FileAnalyzerAgent`'s mount schema expects `child_node` to be a variable name with an `import_source`. A chained `router.routes()` call at the mount site breaks cross-file resolution.

**Fix**: in the mount prompt (`src/agents/file_analyzer_agent.rs:442-447`, "Variable & Alias Resolution"), teach the LLM to walk through `.routes()` / `.middleware()` / `.allowedMethods()` and treat the *receiver* as the child node. This is a prompt-only change.

**Response body**: `ctx.body = users` is an `AssignmentExpression`, not a `CallExpression`. `src/sidecar/src/type-inferrer.ts:445-498` only looks at call expressions. Two options:
- (a) Add a Koa-specific branch that detects `AssignmentExpression` where the left is `ctx.body` / `this.body` and infers from the right.
- (b) Have the `FileAnalyzerAgent` emit `response_expression_text` = `"users"` (the RHS), and rely on the text-based node resolver at `type-inferrer.ts:1354-1366` to find the `Identifier` node and infer from it. The current prompt (`file_analyzer_agent.rs:458-460`) says "res.json(...), reply.send(...), return ..." — add `ctx.body = ...` explicitly.

**Request body**: `ctx.request.body` — generic enough, the inferrer's text-based resolver handles arbitrary expressions.

### 3.2 Fastify

**Producer**: `app.get`, `app.post` — straightforward. `app.register(plugin, { prefix: '/api/v1' })` is the mount pattern. `register` is in `is_potential_api_method` (`swc_scanner.rs:293`), so candidates are emitted. The challenge: the *child* of the mount is a function (the plugin), not a variable — and the prefix is in the second arg, not the first. The LLM needs to know this.

**Fix**: Framework guidance is dynamically generated by `FrameworkGuidanceAgent` (`src/agents/framework_guidance_agent.rs`), so this should just work *if* the LLM correctly describes Fastify's `register` semantics in its response. Worth verifying with a real Fastify fixture.

**Response types**: Fastify's idiomatic style is `async (req, reply) => { return users }` — handled by `inferFunctionReturn` fallback. `reply.send(users)` is in the method whitelist. Both should work.

**Param/body**: `request.body` / `request.params` / `request.query` — inferrer is param-name agnostic.

### 3.3 Hono

**Producer**: `app.get('/users', (c) => c.json(users))` is identical in shape to Express. `app.route('/users', usersApp)` is the mount pattern — `route` is in `is_potential_api_method`. Should work.

**Response types**: `c.json(users)` is in the whitelist. `c.text(x)`, `c.html(x)`, `c.body(x)` are not — adding them is a 1-line fix at `type-inferrer.ts:469, 486`. `return new Response(JSON.stringify(data))` (Web API style) is not handled.

**Param/body**: `await c.req.json()` returns `Promise<unknown>` by default unless the dev uses Zod validators — expected limitation.

**Response-type filter**: the `Hono` class type is not in `is_untyped_response_type`. If a handler's inferred return type resolves to `Hono` (which shouldn't happen for a well-written handler, but could for edge cases), it would leak. Low priority.

### 3.4 NestJS

This is the biggest gap — a hard code change, not a prompt change.

**Producer — architectural miss**: The whole routing surface is decorators on class methods:
```ts
@Controller('users')
export class UsersController {
  @Get(':id')
  async findOne(@Param('id') id: string): Promise<User> { ... }
}
```
SWC parses these correctly, but `src/swc_scanner.rs`'s `CandidateVisitor` doesn't traverse into `Decorator` nodes specifically — it only reacts to `CallExpression`. When `visit_call_expr` fires on the inner `Get(':id')` call, the callee is `Expr::Ident("Get")`, which matches neither the member-call branch nor the hardcoded literal `fetch`. **Result: zero candidates → `should_analyze = false` → file is skipped entirely.**

**Fix options**:
- **(a) Minimal**: add a `visit_decorator` implementation in `CandidateVisitor` that treats decorator calls as candidates, recording the decorator name (Get/Post/Put/Patch/Delete/Options/Head/All) as the "callee_property" and inspecting the containing `ClassMethod` for method name + enclosing `ClassDecl` for controller context.
- **(b) Heuristic**: widen `is_potential_api_method` to treat any bare `Ident` call of `Get|Post|Put|Patch|Delete|Options|Head|All` inside a decorator context as a candidate. Needs to handle the `@Controller('prefix')` class-level decorator to reconstruct the full path.

Either way, the `FileAnalyzerAgent` prompt already says "rely strictly on ACTIVE PATTERNS provided" (`src/agents/file_analyzer_agent.rs:404`), and `FrameworkGuidanceAgent` will happily generate NestJS decorator patterns — but only if the file makes it past the candidate gate.

**Mounts**: NestJS doesn't have Express-style mounts; composition is via `imports: [UsersModule]` in `@Module()` decorators. This maps to the `mount_graph` concept roughly (module imports → path prefixes from `RouterModule.register()` or `@Controller('prefix')`), but it's a different graph shape. Worth a thoughtful design before implementing.

**Request/response types**: `@Body() body: CreateUserDto`, `@Param('id') id: string`, `@Query() query: PaginationDto` — NestJS parameter decorators. Neither the sidecar nor the scanner recognizes these today. `@nestjs/common` wrapper types (`HttpException`, etc.) should be added to `is_untyped_response_type`.

### 3.5 Hapi

**Producer**: `server.route({ method: 'GET', path: '/users', handler: ... })` — the SwcScanner emits a candidate because both `server` and `route` match. The first-arg snippet is an object literal. The LLM must extract `method` and `path` from inside the object. Unverified — worth a Hapi fixture.

Hapi also supports `server.route([{ method, path, handler }, ...])` (array of routes). Same object-literal extraction, just repeated.

**Response**: `h.response(x)` — method name `response` is **not** in `['json', 'send', 'end', 'write']` (`type-inferrer.ts:469`). Bare `return x` (also common in Hapi) works via `inferFunctionReturn` fallback. Fix: add `'response'` to the whitelist, OR emit the fallback explicitly.

**Param/body**: Hapi uses `request.payload` (not `request.body`), `request.params`, `request.query`. The inferrer is param-name agnostic, so the LLM-emitted `payload_expression_text: "request.payload"` should resolve to the right node. Untested.

**Response-type filter**: `Request`, `ResponseToolkit`, `ResponseObject` should be added to `is_untyped_response_type` (`type_sidecar.rs:803-814`).

---

## 4. GraphQL Handling

### 4.1 Current state

- `src/call_site_classifier.rs:37` defines `CallSiteType::GraphQLCall`.
- `src/call_site_classifier.rs:107, 158` — the classification prompt tells the LLM to return `"GraphQLCall"` for GraphQL queries/mutations/subscriptions.
- `src/framework_detector.rs:195, 200` — `graphql-request` is in the data-fetcher detection prompt.

### 4.2 Actual outcome in the current pipeline

The `CallSiteClassifier` is **legacy code** from the pre-file-centric architecture (it's used under `#[allow(dead_code)]`). The active pipeline is `FileAnalyzerAgent` → `FileOrchestrator::build_mount_graph`, and `FileAnalysisResult` has only three buckets: `mounts`, `endpoints`, `data_calls` — no `graphql_calls`.

What actually happens:
- A `graphql-request` call like `client.request(query, variables)` — the SwcScanner might emit a candidate (`.request` is in `is_potential_api_method` at `swc_scanner.rs:302`), and the FileAnalyzerAgent, if told about `graphql-request` in detected data_fetchers, is free to classify it as a `data_call`. But the URL it produces will be the single GraphQL endpoint (e.g., `/graphql`), not per-operation.
- Apollo Client / urql / TanStack Query calls — depending on how they're written, may or may not emit candidates. `useQuery({ query: GET_USERS })` is a React hook, not a recognizable HTTP pattern.

**So GraphQL is "silently ignored" rather than "cleanly detected and skipped".** It won't crash; it just won't show up as an endpoint in the report.

### 4.3 Recommendation

Add a shallow GraphQL detection pass with a clear user-facing message rather than deepening the scanner.

**User-facing message shape** (shown once per repo in the scan report):
```
ℹ  GraphQL usage detected in this repo (apollo-server, graphql-request, urql, @apollo/client, …)
   Carrick v1 analyzes REST contracts only. GraphQL schema drift and resolver
   type checking are not yet supported — GraphQL calls are listed below without
   contract analysis:

     - src/pages/users.tsx:42      useQuery(GET_USERS)
     - src/pages/orders.tsx:17     client.request(CREATE_ORDER, variables)

   REST endpoints in this repo were analyzed normally.
```

**Implementation**:
- Detect GraphQL *usage* (not per-call) from `framework_detector.rs` detected data-fetchers: if any of `graphql`, `graphql-request`, `@apollo/client`, `@apollo/server`, `urql`, `@urql/core`, `relay-runtime`, `@tanstack/react-query` (with `gql` queries), `graphql-tag` is in the manifest, print the banner once.
- List call sites by grep-matching candidate targets whose `path_snippet` looks like a GraphQL operation (`query`, `mutation`, `subscription` template literals, or known hook names) — don't try to infer types.
- No changes to the core graph. Keep it strictly informational.

This keeps the scope commitment (REST only for v1) while giving users a clear signal and an obvious upgrade hook ("GraphQL support coming — get on the waitlist").

---

## 5. TSX Coverage

### 5.1 Confirmed walked

- `src/file_finder.rs:103` — `matches!(ext_str.as_str(), "js" | "ts" | "jsx" | "tsx")` for the main walk.
- `src/engine/mod.rs:337` — same filter for the incremental (git-diff) code path.
- `src/parser.rs:19` and `src/swc_scanner.rs:158` — SWC's TypeScript syntax mode is enabled for both `.ts` and `.tsx`.
- `src/sidecar/src/bundler.ts:690` — bundler's extension list: `['.ts', '.tsx', '.js', '.jsx', '.d.ts']`.

**.tsx is walked for consumer extraction.** This is the intended behavior for React components that call REST APIs.

### 5.2 Edge case where `.tsx` is silently handled suboptimally

**`src/agents/file_orchestrator.rs:983-988`** — import resolution defaults to `.ts`:

```rust
let with_extension = if !resolved_str.ends_with(".ts")
    && !resolved_str.ends_with(".tsx")
    && !resolved_str.ends_with(".js")
    && !resolved_str.ends_with(".jsx")
{
    format!("{}.ts", resolved_str)       // defaults to .ts
}
```

When the `FileAnalyzerAgent` reports a mount like:
```
mount: app.use('/ui', import('./pages/UserPage'))   // import_source = './pages/UserPage'
```
…the orchestrator canonicalizes to `./pages/UserPage.ts`. If the file is actually `./pages/UserPage.tsx`, the canonicalize fails, and the mount edge lands on a non-existent node. Cross-file linking breaks silently.

This is rare for producers (no one defines Express routers in `.tsx`) but could bite in Next.js-shaped codebases that export route handlers from `.tsx` files (unusual but not unheard of).

**Fix**: try `.ts`, `.tsx`, `./index.ts`, `./index.tsx` in order; use the first one that exists.

### 5.3 `.tsx` for producer vs consumer

The task brief asked for `.ts`-only producer walks. The current code is uniform — one walk, both producer and consumer extraction run on every `.ts/.tsx/.js/.jsx` file. This is not a gap; producers defined in `.tsx` are rare enough that the false-positive cost is negligible, and splitting walks would complicate incremental mode for no real win.

---

## 6. Suggested Test Repos

Vetted against TS + REST + multi-service + active development. For each, "surface" = how many distinct services/apps to analyze, and known caveats.

### 6.1 Priority candidates for teardown blog posts

Verified via GitHub on 2026-04-17.

| Repo | Framework (verified) | TS % | Monorepo | REST/GraphQL | Verdict |
|---|---|---|---|---|---|
| **`medusajs/medusa`** | Express `^4.21.0` (confirmed from `packages/medusa/package.json` on `develop`) | 86.1% | ✅ `packages/` | REST | **Ship the Express teardown here.** Largest well-typed Express OSS. |
| **`directus/directus`** | Express (confirmed from `api/package.json`, no other framework present) | 77.0% (+ 21.5% Vue frontend) | ✅ `/api`, `/app`, `/sdk`, `/packages`, `/directus` | REST + GraphQL | ✅ Use as Express alt. Cleaner than Medusa but smaller service graph. |
| **`twentyhq/twenty`** | NestJS (confirmed via page stack) | 78.7% | ✅ Nx monorepo, `packages/` | **GraphQL primary**, REST secondary | **Defer until NestJS gap is closed.** Will surface flagged-GraphQL + some REST — good demo of the v1 scope boundary. |
| **`novuhq/novu`** | NestJS 10.4.18 + `@nestjs/platform-express` (Express 5.0.1 as platform adapter — confirmed from `apps/api/package.json`) | 96.6% | ✅ `apps/api`, `apps/web`, `apps/dashboard`, `apps/ws`, `libs/`, `packages/` | REST | **Best NestJS teardown candidate** once decorator support lands. Deepest microservice graph of any candidate. |
| **`strapi/strapi`** | Koa (confirmed — still Koa in v5.x, page topics explicitly list `koa` / `koa2`) | 85.9% | ✅ `packages/` | REST + GraphQL plugin | ✅ after Koa `ctx.body =` fix. |
| **`payloadcms/payload`** | **Next.js only** — no Express/Koa/Fastify in current `packages/payload/package.json`. My earlier "Express + GraphQL" claim was stale. | (high) | ✅ `packages/` | REST + GraphQL | Treat as Next.js API-route candidate, not an Express one. Usable but shape-shifts toward Cal.com's pattern. |
| **`n8n-io/n8n`** | Express (not re-verified, but well-known) | high | ⚠ monolith-ish | REST | Too big for a clean teardown post. Skip for v1. |
| **`calcom/cal.com`** | Next.js API routes + tRPC (not re-verified) | high | ✅ `apps/web`, `apps/api-v1`, `apps/api-v2` | REST + tRPC | `apps/api-v2` is tRPC-derived — Carrick won't analyze that cleanly. `apps/api-v1` and `apps/web` API routes are the target surface. |

### 6.2 Secondary / lower priority

- **Formbricks** (`formbricks/formbricks`) — Next.js, TS monorepo. Solid but smaller than Cal.com.
- **Ghost** (`TryGhost/Ghost`) — Express, large, but legacy patterns and partial TS adoption. Skip.
- **Rocket.Chat** — Meteor-based, non-standard. Skip.
- **Supabase** (`supabase/supabase`) — polyglot (Deno + Node). Partial fit. Skip for v1.

### 6.3 Fastify / Hono / Hapi candidates

Active multi-service OSS in TypeScript is thin in these communities:
- **Fastify**: `platformatichq/platformatic` uses Fastify as its core — but the codebase is CLI-heavy rather than service-heavy. Workable but not blog-post-juicy.
- **Hono**: mostly single-service Workers apps. No great multi-service OSS candidate right now — prioritize documentation/synthetic fixtures instead.
- **Hapi**: the ecosystem has contracted. `hapijs/hapi` itself is TS but the broader OSS surface is sparse. Defer.

For Fastify/Hono/Hapi, build hand-crafted multi-service fixtures under `tests/fixtures/` and exercise the pipeline against those before investing in external teardowns.

### 6.4 REST + GraphQL mixed repos (still viable)

Carrick's REST surface analyzes cleanly; GraphQL gets flagged. These are fine teardown targets:
- **PayloadCMS** — REST primary, GraphQL plugin
- **Strapi** — REST primary, GraphQL plugin
- **Twenty** — GraphQL primary, **REST secondary** (auth, webhooks). The blog angle becomes "here's the REST surface Carrick found; GraphQL next".

---

## 7. Prioritized Workstream

Close gaps in this order. Each step unblocks a specific growth-playbook dependency.

### Step 1 — Koa `ctx.body` + Hapi `h.response` response detection
**Effort**: ~1–2 days. Add `'response'` to the response-body method whitelist in `src/sidecar/src/type-inferrer.ts:469, 486`. Add an `AssignmentExpression` branch to `resolveResponsePayloadNode` that detects `ctx.body = x` / `this.body = x`. Update the `FileAnalyzerAgent` system prompt at `src/agents/file_analyzer_agent.rs:458-460` to explicitly mention `ctx.body = ...` and `h.response(...)` as response-emission patterns.

**Unlocks**: Koa teardown (Strapi). Hapi teardown (synthetic fixture).

### Step 2 — Wire the orphan fixtures into real tests
**Effort**: ~1 day. `tests/fixtures/fastify-api/server.ts` and `tests/fixtures/koa-api/server.ts` exist but no Rust test references them. Add a `tests/framework_coverage_test.rs` that scans each fixture end-to-end and asserts the expected endpoint count, paths, and that response types resolve (or resolve to `unknown` in the framework-type case).

**Unlocks**: Fastify and Koa are no longer "code-present-untested". Creates regression guardrails before OSS teardowns go public.

### Step 3 — NestJS decorator support
**Effort**: ~1 week. Add `visit_decorator` to `CandidateVisitor` in `src/swc_scanner.rs` that emits candidates for decorator calls matching `Get|Post|Put|Patch|Delete|Options|Head|All`. Teach the `FileAnalyzerAgent` prompt that decorator patterns mean "the enclosing class method is the handler" and "the enclosing `@Controller('prefix')` provides the path prefix". Add NestJS-specific types to `is_untyped_response_type`.

**Unlocks**: Twenty teardown. Novu teardown. Without this, ~30% of the serious Node backend OSS surface is invisible to Carrick.

### Step 4 — GraphQL "detected but out-of-scope" banner
**Effort**: ~2 days. Detect GraphQL libs in `framework_detector.rs` output, add a banner to the report formatter (`src/formatter/`). List the candidate sites without type analysis. Wire to the growth playbook's GraphQL-is-next pitch.

**Unlocks**: Clean story for PayloadCMS / Strapi / Twenty teardowns (all mix REST + GraphQL).

### Step 5 — Lower-priority polish
- Hono: add `'text'`, `'html'`, `'body'` to the response-method whitelist.
- Import resolution (`file_orchestrator.rs:983-988`): try `.ts` → `.tsx` → `./index.ts` → `./index.tsx` in order.
- Widen NestJS/Hapi/Hono types in `is_untyped_response_type`.
- Add Hono to the `framework_detector.rs` example list at line 194.

> **Framing**: Steps 1–3 are the first moves toward the target architecture in §9 (rules-driven pipeline). Step 1 eliminates one heuristic outright. Steps 2–3 move two more heuristics out of Rust source and into LLM-generated rule data. Keep that direction of travel in mind — every fix in this workstream should either delete framework-specific code or replace it with consumption of LLM-generated rules, never add new hardcoded names.

---

## 8. Honest Summary for the Growth Playbook Dependency

Can Carrick claim "framework-agnostic REST" today?

- **No** — not credibly. Express is the only framework with real test coverage. Koa has a structural response-body gap. NestJS is architecturally invisible to the scanner.
- **But the architecture is sound.** The LLM-driven `FrameworkGuidanceAgent` → `FileAnalyzerAgent` path genuinely adapts to any framework the LLM knows about. The deterministic scaffolding (candidate scanner, response-body heuristic, hardcoded framework name lists) is the only thing coupled to Express.

**Realistic timeline to credibly close the gap**: Steps 1–3 above ≈ 2 weeks of focused work. After step 3, Carrick is honestly "works on Express, Koa, Fastify, Hono, NestJS with real validation; Hapi coverage is code-present-untested".

**What to do about the playbook in the meantime**: ship the Express teardown first (Medusa or Directus). Start Step 1 in parallel. Hold NestJS teardown candidates (Twenty, Novu) until Step 3 is done — a failed OSS teardown post hurts more than a delayed one.

---

## 9. Direction of travel — toward framework-agnostic, at MVP scope

The goal isn't zero heuristics. Frameworks have genuinely distinct shapes (`res.json` vs `ctx.body =` vs `@Get()`) and pretending otherwise produces worse output. The goal is: **minimise the maintenance burden when frameworks ship new versions or change their API surface, while delivering something devs actually find useful today.**

This is an MVP-stage tool. The principles below are the direction of travel, not a production architecture.

### 9.1 The key insight — guidance IS the rules

The pipeline already has an LLM-driven guidance layer:

- `FrameworkDetector` (`framework_detector.rs:86-148`) identifies frameworks from `package.json` + imports
- `FrameworkGuidanceAgent` (`agents/framework_guidance_agent.rs`) generates patterns, triage hints, parsing notes per detected framework, every scan
- `FileAnalyzerAgent` consumes that guidance when analysing each file

**That guidance output *is* the rules.** No separate artifacts, no bundled JSON, no cache dir, no community rules repo. The LLM regenerates what it needs per scan — Express 4 vs Express 5, Fastify 3 vs 4 — because detection reads `package.json` which names the version. The agent pipeline already handles this; we just haven't leaned on it.

The maintenance-reducing move is to **make the guidance output richer and the deterministic scaffolding thinner** — not to build a rules-generation system on the side.

### 9.2 What "heuristic" means here

Two categories get conflated; only one is the maintenance problem:

- **Heuristic in Rust/TS source** — framework-specific lists like `["json", "send", "end", "write"]` at `type-inferrer.ts:469`, hardcoded framework names at `type_sidecar.rs:797-815`, the `"res"`/`"req"` identifiers in `extractor.rs`. Maintainers edit these when frameworks change. **This is the burden to eliminate.**
- **LLM guidance output** — regenerated per scan from the detected framework + version. Carrick's source stays framework-neutral; the per-scan LLM calls absorb the variation. **This is fine — it's data, not code.**

Every move below either kills a source-level heuristic outright, or shifts one from source into the guidance layer where it already lives.

### 9.3 The MVP moves

These collapse the three refactors I previously sketched. In priority order:

**Move 1 — kill the response-method whitelist.** Change what the `FileAnalyzerAgent` emits. Today: `response_expression_text: "res.json(users)"`, and `type-inferrer.ts:469` hardcodes `["json", "send", "end", "write"]` to reach in and grab the argument. Tomorrow: the LLM emits `response_payload_expression_text: "users"` directly — the thing whose type we actually want. Sidecar calls `type.getText()` on that node, no list, no drill-in logic. This collapses Express `res.json(x)`, Koa `ctx.body = x`, Hapi `h.response(x)`, Fastify/NestJS/Hono bare `return x`, all into one path. **Pure elimination, no rule generation involved.**

Edge case to handle: payload-less endpoints (redirects, 204s, streaming). LLM emits `null`; sidecar infers from the enclosing expression as a fallback. One `if` branch in the sidecar, not a new subsystem.

**Move 2 — widen the candidate scanner.** Today `SwcScanner` hardcodes "emit candidates for member calls and the bare identifier `fetch`." NestJS files get 0 candidates (§2.3, test-verified). Rather than building a rule-consumption system to make the scanner framework-aware, **widen the net**: emit candidates for all call expressions and for decorator calls. The LLM does more filtering work; the scanner does less. NestJS works immediately. Rust source contains zero framework names.

The cost is more LLM tokens per scan. That's the right trade at MVP scale — LLM cost scales predictably, maintenance burden doesn't.

**Move 3 — use imports that already exist.** `ImportSymbolExtractor` (`visitor.rs:185-235`) tracks local-name → source mapping for every import. This data doesn't currently reach `FileAnalyzerAgent`'s per-file prompt. Pipe it in as structured context:

```
This file imports:
  - Get, Post, Body, Controller from '@nestjs/common'
  - UserService from './user.service'
```

The LLM now has explicit grounding per file — it knows `@Get()` in this file means the NestJS decorator, not a local function called `Get`. It knows which candidates to treat as framework-native. No rule-keying scheme, no per-file selection algorithm — just richer prompt context from data the AST already produced.

This is the single highest-leverage change: it grounds the LLM against the actual repo, not against a pattern list.

### 9.4 Wrapper types — a later move, same shape

`wrapper_registry.rs:8-20` hardcodes axios. The TS-side `ExtractionConfig` (`type-inferrer.ts:696-722`) already supports a generic rule shape. For MVP, have `FrameworkGuidanceAgent` emit extraction config for the detected data-fetchers as part of its regular output — the axios config if axios is present, the `@nestjs/axios` `Observable<AxiosResponse<T>>` config if that's present, etc. Sidecar consumes whatever it's given.

No separate rules pipeline. Just one more field in guidance output.

### 9.5 The MVP pipeline

```
[package.json + repo-wide imports]
  ↓
FrameworkDetector — identifies frameworks + versions from deps
  ↓
FrameworkGuidanceAgent — emits patterns, wrapper configs, triage hints (per scan, already exists)
  ↓
SwcScanner (widened) — emits candidates for all calls + decorators, framework-blind
  ↓
FileAnalyzerAgent — receives guidance + per-file import table + candidates
  ↓  emits richer schema: payload expressions, wrapper info, explicit framework context
  ↓
Sidecar — generic consumer, no framework-specific branches
  ↓
MountGraph + type resolution — already framework-agnostic
```

**What changes when a framework ships a new version:** nothing in Carrick's source. `FrameworkDetector` sees the new version in `package.json`; `FrameworkGuidanceAgent` asks the LLM for guidance on that version; downstream code consumes whatever comes back. The LLM's knowledge cutoff is the only failure mode, and it's rare (frameworks don't release breaking majors often).

### 9.6 Honest limits at MVP scope

- **LLM knowledge cutoff.** If a framework ships v-next tomorrow with breaking changes, the LLM's guidance might be stale. At MVP this is documented-not-mitigated — users on bleeding-edge versions may see degraded output. Revisit if it becomes a real complaint.
- **NestJS response transformers** (`@SerializeOptions`, `ClassSerializerInterceptor`, response DTOs) change the outgoing type in ways that raw return-type inference misses. Documented limitation for v1: "Carrick infers raw handler return types; NestJS interceptor/serializer transforms are not applied." Honest limit, not a bug.
- **Correctness of LLM guidance is unverified.** The orphan fixtures (`tests/fixtures/fastify-api/`, `koa-api/`) + the NestJS decorator test + a synthetic Hapi fixture are the MVP coverage. If guidance drifts silently, those fixtures catch it. Per-rule correctness testing is a post-MVP concern.
- **Move 2 trades tokens for simplicity.** Widening the scanner means more LLM work per scan. At MVP scale (free tier is rate-limited per §playbook), this is acceptable. At production scale it's worth measuring.

### 9.7 Relationship to the workstream in §7

Map of the section-7 steps to the moves here:

- Step 1 (Koa `ctx.body` + Hapi `h.response`) = **Move 1**. Not a whitelist extension — the actual MVP fix is the payload-expression schema change, which makes both cases disappear together.
- Step 2 (wire orphan fixtures into tests) = precondition for all three moves. You can't tell if the pipeline regressed without them.
- Step 3 (NestJS decorator support) = **Move 2**. Widening the scanner gets NestJS for free; decorators are no longer special.
- Step 4 (GraphQL banner) = orthogonal. Keep as-is.
- Step 5 (lower-priority polish) = **Move 3** touches most of these (Hono method list disappears if Move 1 lands; import resolution improves with richer import context in Move 3).

Section 7 is the sequence to ship for the growth playbook's OSS teardowns. Section 9 is why each step points in the same direction.

---

## 10. Framework fixtures in CI

Catch guidance drift and cross-framework regressions by running the **published** Carrick binary against a directory of realistic per-framework fixtures on every PR. This is the lightest possible infrastructure that gives the §9 moves a safety net.

### 10.1 Directory layout

```
tests/framework-fixtures/
  express-4/              { server.ts, client.ts, package.json, expected.json }
  express-5/
  koa-2/                  (wire up the orphan at tests/fixtures/koa-api/)
  fastify-4/              (wire up the orphan at tests/fixtures/fastify-api/)
  hono-4/
  nestjs-10/              (gate on §7 Step 3 landing)
  hapi-21/

  multi-service-express/         repo-a + repo-b, producer/consumer split
  multi-service-nestjs-koa/      mixed framework, cross-repo
  react-tsx-consumer/            tsx file calling REST APIs
```

Each fixture is **minimal but realistic** — 3–5 endpoints covering:
- GET without body
- GET with path param (`:id`)
- POST with request body (exercises request-type inference)
- the framework's mount/plugin/register/route-composition idiom
- one data-fetching call in a separate file (exercises consumer extraction)

Pinned framework version in `package.json`. No transitive deps beyond the framework itself.

### 10.2 What `expected.json` asserts

Structural invariants, not exact strings. LLM wording varies run-to-run; methods, paths, and type identifiers don't.

```json
{
  "min_endpoints": 3,
  "endpoints_contain": [
    { "method": "GET",  "path": "/users" },
    { "method": "GET",  "path": "/users/:id" },
    { "method": "POST", "path": "/orders" }
  ],
  "data_calls_contain": [
    { "method": "GET", "target_contains": "/comments" }
  ],
  "types_resolved": {
    "GET /users": { "response_contains": "User" }
  }
}
```

Predicates (`_contains`, `min_`) tolerate benign variation. Exact-match assertions on handler names or error messages will flake — don't use them.

### 10.3 CI shape

GitHub Action `matrix` over fixture directories. Per fixture:
1. Install the **published** Carrick (`npx carrick` or whatever the release surface becomes), not a `cargo build` from the PR branch. This catches packaging bugs (missing sidecar, wrong defaults) that unit tests miss.
2. Run `carrick scan .` in the fixture dir.
3. Diff the output against `expected.json` with a small harness (Node or Python, whichever's already in the repo toolchain).
4. Fail the build on predicate mismatch.

Exception: the `multi-service-*` fixtures need cross-repo analysis, which requires the hosted pipeline (`CARRICK_API_KEY`, upload to cloud storage). Gate those behind a CI secret rather than running them per-PR — nightly is fine.

### 10.4 Handling LLM nondeterminism

Two layers, in order:
1. **Predicate assertions** (§10.2). Start here. Tolerates variation; catches real regressions.
2. **Response caching** — only if predicate-level tests still flake. Record agent-proxy responses per fixture, replay in CI. The caching boundary is the agent-proxy (already the LLM choke point), so no scanner changes.

Don't build (2) speculatively. It's a "we have flakes" problem, not a "we might have flakes" problem.

### 10.5 Starting order

1. `express-4/` — port from the existing `scenario-1` fixture to the new format; sanity-check the harness.
2. `koa-2/` + `fastify-4/` — wire up the orphans (`tests/fixtures/koa-api/server.ts`, `tests/fixtures/fastify-api/server.ts`). Document what fails; those are the §7 Step 1 fixes.
3. `hono-4/` — write from scratch, small.
4. `nestjs-10/` — write, merge once §7 Step 3 (Move 2) lands.
5. `hapi-21/` — last.

Then the `multi-service-*` fixtures, which is where Carrick's distinctive cross-repo value gets exercised. Per-framework fixtures catch guidance drift; multi-service fixtures catch graph-builder and URL-normalizer bugs.

### 10.6 Versioning

One version per framework at MVP. Directory naming (`express-4/`, `express-5/`) leaves room for more but doesn't obligate building them. Add a second version when (a) the framework ships a major, or (b) a user reports a version-scoped bug. Don't pre-build a matrix.

### 10.7 Deferred concerns

- **Free-tier rate limiting against CI** — per §growth-playbook, anonymous scans are 1/day/IP. CI running the published binary will trip this once fixtures multiply. Solve it when it happens: either a `CARRICK_API_KEY` allocated to the CI runner, or the response caching in §10.4. Not a day-one concern.
- **Fixture maintenance as frameworks evolve** — a fixture pinned to `fastify@4.26.0` will drift when Fastify 5 is common. Acceptable drift; update fixtures when the drift masks real bugs, not on a schedule.
