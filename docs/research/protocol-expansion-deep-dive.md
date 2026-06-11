# Protocol expansion deep dive: GraphQL, WebSockets, and beyond REST

**Status:** research note, June 2026. No code changes; this assesses the lift to take
Carrick from REST-only to multi-protocol contract indexing, with compiled-artifact
consumers (React Relay, persisted queries) explicitly out of scope.

## TL;DR

The architecture is closer to multi-protocol than "REST scanner" suggests. The type
sidecar is already protocol-agnostic, framework knowledge is injected at runtime by
LLM-generated guidance rather than hardcoded, and the SWC candidate scanner already
*detects* `new WebSocket(...)` / `new EventSource(...)` (it just refuses to analyze
them). The REST coupling is concentrated in one assumption repeated across ~6 layers:
**an operation is identified by `(method: String, path: String)`**.

The work splits into:

1. **A one-time foundation refactor** — replace `(method, path)` with a
   protocol-tagged operation key everywhere it appears. Moderate and mechanical. It
   touches the cloud contract, but with no users (per repo rules) that just means
   changing both repos in lockstep — no compatibility ceremony.
2. **Per-protocol extraction + matching + compatibility modules** — mostly *new* code
   hanging off the foundation, not rewrites. GraphQL is the standout: its contract is
   declared (SDL + documents), so extraction is deterministic parsing and
   compatibility checking is the GraphQL validation algorithm — *more* precise than
   the TS-assignability pipeline REST needs, and largely independent of it.

Rough sequencing by value-for-effort: foundation → GraphQL (queries/mutations) →
Socket.IO/typed WebSockets → async messaging (Kafka/SQS/BullMQ) → gRPC/tRPC.

---

## 1. Where REST is actually baked in today

The product loop is: extract **producers** (endpoints) and **consumers** (outbound
calls) per repo → upload `CloudRepoData` → merge all repos → match consumers to
producers → check type compatibility → report drift. Every stage keys operations on
HTTP method + path:

| Layer | Coupling | Location |
|---|---|---|
| LLM file analysis output | `EndpointResult { method, path, ... }`, `DataCallResult { method, target, ... }` | `src/agents/file_analyzer_agent.rs` |
| Mount graph | `ResolvedEndpoint { method, path, full_path }`, `DataFetchingCall { method, target_url }`; path resolution = Express-style prefix joining | `src/mount_graph.rs:35-55` |
| Stored index | `ApiEndpointDetails { route, method, request_*, response_* }` — one struct for both endpoints and calls | `src/analyzer/mod.rs:80-98` |
| Type manifest | entries keyed `(method, path, role, type_kind)` where `type_kind ∈ {Request, Response}` | `src/cloud_storage/mod.rs:29-99` |
| Matching | `MountGraph::find_matching_endpoints_with_normalizer(route, method, ...)`; `UrlNormalizer` assumes http(s) URLs, env-var bases, path params | `src/analyzer/mod.rs:896-1026`, `src/url_normalizer.rs` |
| Formatter | regex-extracts `GET\|POST\|PUT\|DELETE\|PATCH` + path from issue strings; renders the "GraphQL detected — v1 analyzes REST contracts only" banner | `src/formatter/mod.rs:150-168, 250-310` |
| Cloud payload | `CloudRepoData { endpoints, calls, mounts, type_manifest, ... }` — shape mirrored by carrick-cloud index + MCP tools | `src/cloud_storage/mod.rs:102-140` |

What is **not** REST-coupled, and matters a lot for the lift estimate:

- **The type sidecar** (`src/sidecar/`) answers "what is the TypeScript type at this
  span / of this function's return / of this call's result". Its `InferKind` set
  (`response_body`, `call_result`, `function_param`, ...) and the `ExtractionConfig`
  wrapper-unwrapping system are generic. A Socket.IO handler payload or a GraphQL
  resolver return is just another span to it.
- **Framework knowledge** arrives via the cloud (`/framework-detect`,
  `/framework-guidance` → pattern lists consumed by `file_analyzer_agent`). Adding
  recognition of `graphql-yoga` or `socket.io` is prompt/guidance work in
  carrick-cloud, not scanner rewrites.
- **The SWC candidate scanner** is a broad net; it already emits
  `WebSocket`/`EventSource`/`XMLHttpRequest` constructions as candidates
  (`src/swc_scanner.rs:698-705`) and `CallSiteType::GraphQLCall` exists as a
  vestigial enum variant (`src/call_site_classifier.rs:43`).
- **Cross-repo merge + dependency conflict + intent/function indexing** — protocol
  irrelevant.

## 2. The generalization: typed operation contracts

Everything Carrick checks reduces to:

> A **producer** declares an operation with an input type and an output type. A
> **consumer** invokes an operation expecting an input/output type. Operations match
> by a **protocol-specific key**; compatibility is **directional type containment**
> (producer output ⊆ consumer expectation; consumer input ⊆ producer expectation).

The foundation refactor introduces this explicitly:

```rust
enum Protocol { Http, Graphql, Websocket, Sse, Queue, Grpc, Trpc }

// Canonical, serializable operation identity. The HTTP case is exactly today's key.
enum OperationKey {
    Http     { method: String, path: String },
    Graphql  { kind: GqlKind /* Query|Mutation|Subscription */, field: String },
    Socket   { namespace: Option<String>, event: String, direction: Direction },
    Queue    { topic: String, direction: Direction },
    Grpc     { service: String, method: String },
}
```

- `ApiEndpointDetails.{method,route}` → `key: OperationKey` (+ keep an
  `operation_id: String` canonical serialization for index keys and manifest
  aliases). Per the repo's no-backwards-compat rule this is a delete-and-replace, not
  a parallel path.
- `TypeManifestEntry.{method,path}` → `operation_id` + `protocol`;
  `ManifestTypeKind {Request, Response}` generalizes to input/output and both become
  optional (one-way messages have no response; GraphQL types may bypass the TS
  manifest entirely, see §3).
- Matching becomes a per-protocol dispatch: `MountGraph` remains the HTTP matcher;
  GraphQL/socket/queue matchers are simple exact/structured lookups (no mount
  hierarchy, no URL normalization — *easier* than HTTP).
- Formatter findings generalize cleanly: *missing endpoint* → "operation invoked but
  not provided", *orphaned endpoint* → "operation provided but never invoked",
  *method mismatch* → key mismatch, all rendered with protocol-specific phrasing.
  The parsing-findings-back-out-of-strings approach in
  `formatter/mod.rs:250-310` should be replaced with structured findings as part of
  this (it's already fragile).

**The refactor spans two repos, but with no users it carries no compat cost.**
`CloudRepoData` is the wire contract with carrick-cloud: the index schema,
`get_api_endpoints` / `get_endpoint_types` / `check_compatibility` MCP tools, and the
file-analysis prompts all mirror the `(method, path)` shape and live in the other
repo. Per the no-users/no-backwards-compat rule, there is no capability flag or
staged deploy: change both repos in lockstep, redeploy, bump `CACHE_VERSION`, and let
per-file caches and the index rebuild on the next scan. Framework-guidance and
file-analyzer prompt changes are also carrick-cloud work (prompt-leak guard keeps
them out of this repo); only the response-schema structs in `src/agents/schemas.rs` /
`file_analyzer_agent.rs` change here.

## 3. GraphQL (queries + mutations first)

GraphQL is the highest-demand protocol and, counterintuitively, has the *most
tractable* precise checking story, because the contract is declared rather than
inferred:

**Producer extraction — get the SDL.**
- SDL-first servers (Apollo Server, graphql-yoga, mercurius): `typeDefs` as
  `gql\`...\`` / template literals, `.graphql`/`.gql` files, or a committed
  `schema.graphql` artifact. All statically readable; parse with `apollo-rs` (Rust)
  or `graphql-js` in the existing Node sidecar.
- Code-first servers (Pothos, TypeGraphQL, Nexus): the schema exists only after
  executing user code, which we won't do. Pragmatic v1 stance: **support SDL
  artifacts only**, and detect code-first builders to emit a one-line suggestion to
  commit the emitted schema (most code-first projects already generate
  `schema.graphql` via codegen/CI; a `carrick.json` key can point at it). LLM
  reconstruction of code-first schemas is possible later but lossy — don't lead with
  it.

**Consumer extraction — get the documents.** `gql` tagged literals and `.graphql`
files cover graphql-request, Apollo Client, urql, and raw `fetch` with a query
string. Crucially, **graphql-codegen consumers stay in scope**: the source documents
exist even when types are generated. What's out of scope is exactly what the product
brief expects: Relay (compiler replaces documents with artifacts) and persisted-query
manifests (documents replaced by hashes).

**Matching + compatibility.** A consumer document is checked by running **GraphQL
validation of the document against the producer SDL** (field existence, argument
types, selection validity, deprecations). This is the standard algorithm — run
`graphql-js` `validate()` inside the existing sidecar process (Node is already in the
toolchain) or `apollo-rs` validation in Rust. It is *more precise* than TS
assignability and sidesteps the type manifest / bundled-`.d.ts` pipeline for GraphQL
entirely. The TS sidecar remains useful only for the optional later check that
variable *values* constructed in TS match declared variable types.

**Attribution.** Everything lives at one `/graphql` URL, so with multiple GraphQL
services in an org, which schema does a document validate against? Reuse the existing
env-var/domain classification (`carrick.json` `internalEnvVars`/`internalDomains`) on
the client's endpoint URL, fall back to "validate against every known schema, report
the best match". Apollo Federation is the known complication (documents valid against
the supergraph, not any one subgraph) — punt to a later phase, banner it like GraphQL
is bannered today.

**Lift: medium.** Mostly new, self-contained code (SDL/document parsing, a validation
call, a matcher) on top of the foundation; the existing REST pipeline is untouched.
Subscriptions ride the WebSocket model below.

## 4. WebSockets, Socket.IO, SSE

Two very different problems wearing one name:

- **Socket.IO (and typed wrappers)** is tractable and worth doing first-class. There
  *is* an operation key: `(namespace, event name, direction)`.
  `socket.on("event", handler)` is a producer of `client→server` handling;
  `socket.emit("event", payload)` is a consumer; acks give a response type.
  Socket.IO's idiomatic `ServerToClientEvents`/`ClientToServerEvents` interfaces are
  pure gold when present — the sidecar can lift the whole contract from one
  interface. The existing env-var/domain machinery classifies the `io(url)`
  connection target. Direction-aware matching means findings like "event emitted by
  service A but no listener in service B".
- **Raw `ws`/`WebSocket`** has *no protocol-level operation key — messages are opaque
  frames demultiplexed by app convention (`switch (msg.type)`). Best effort: LLM
  extracts the discriminant convention + the sidecar types the discriminated union;
  confidence-label these findings. Do not promise drift detection here.
- **SSE / EventSource**: model as one-way server→client named events; rides the
  socket model. Server side (`res.write("event: ...")`) is convention-soup — start
  consumer-side only.

**Lift: medium** for Socket.IO (new extraction patterns via framework guidance, new
key variant, direction handling in matcher + manifest where request/response become
optional). Raw-ws best effort is mostly prompt + labeling work.

## 5. Async messaging (Kafka, SQS, BullMQ, EventEmitter-style buses)

Flagging this even though the prompt says "GraphQL, web sockets etc": for *real-world
systems* this may be the highest drift-detection value per unit effort. Queue
payloads are stringly-typed, invisible to compilers, and break silently — exactly the
gap Carrick exists to fill. The model fits perfectly: key = `(topic/queue,
direction)`, matching = literal string lookup (no URL normalization at all), types =
sidecar on the publish payload and consumer handler param, findings = "topic produced
but never consumed" / payload type drift. Extraction needs per-client guidance
(kafkajs, `@aws-sdk/client-sqs`, bullmq) — same LLM-guidance channel as REST
frameworks. **Lift: low-medium once the foundation exists.**

## 6. gRPC and tRPC (defer)

- **gRPC**: contract is `.proto` — deterministic parsing, key =
  `(package.Service, method)`. But TS gRPC traffic is rarer, protos often live in a
  shared registry repo (a new acquisition problem), and drift checking is
  proto-vs-proto comparison — a third compatibility engine. Defer.
- **tRPC**: the contract *is* a TS type, and the TS compiler already enforces it
  within the monorepos where tRPC lives; cross-repo tRPC is rare. Drift checking is
  mostly redundant — but *indexing* procedures (for MCP `search_by_intent` /
  `get_api_endpoints`) is cheap and useful. Index-only support, no checking.

## 7. Component-by-component lift summary

| Component | Change | Lift |
|---|---|---|
| Data model (`ApiEndpointDetails`, manifest, `CloudRepoData`) | `OperationKey`/`Protocol`, optional input/output | Medium, mechanical, one shot |
| carrick-cloud contract (index, MCP tools, prompts) | Mirror the above; lockstep redeploy; `CACHE_VERSION` bump (no compat needed — no users) | Medium, mechanical |
| SWC scanner | Stop suppressing WS/GQL candidates; add `gql` tag / `.graphql` / socket patterns | Low |
| LLM schemas + guidance | Extend `FileAnalysisResult` with per-protocol result arrays; prompts in carrick-cloud | Medium |
| Mount graph / matching | Keep for HTTP; add trivial exact-key matchers per protocol | Low |
| Type sidecar | New `InferKind`s + extraction rules (Apollo result `.data`, socket payloads) | Very low |
| Compatibility checking | GraphQL: document-vs-SDL validation (new, self-contained). Sockets/queues: existing ts_check pipeline | Medium (GraphQL), low (rest) |
| Formatter | Structured findings + per-protocol phrasing; retire GraphQL banner | Low-medium |

## 8. Suggested phasing

1. **Phase 0 — foundation** (prerequisite tax): `OperationKey` refactor across
   scanner + lockstep carrick-cloud schema/tooling change. The only step that touches
   the working REST path; risk is ordinary regression risk, covered by the
   `examples/` e2e fixtures and the fixture-driven mock-LLM pipeline harness
   (`CARRICK_MOCK_FIXTURE_DIR`, `tests/llm_mock_pipeline_test.rs`). Everything after
   it is additive. Don't do it speculatively — land it in the same arc as Phase 1.
   Each subsequent protocol phase should ship with its own mock-LLM fixture project,
   so extraction → matching → checking is regression-tested end to end without live
   LLM calls.
2. **Phase 1 — GraphQL queries/mutations**: SDL + document parsing, validation-based
   checking, attribution via existing domain classification. Out of scope: Relay,
   persisted queries, federation composition, code-first without an SDL artifact.
3. **Phase 2 — Socket.IO + typed WebSockets** (+ SSE consumer-side, GraphQL
   subscriptions on the same direction-aware model). Raw `ws` as labeled best-effort.
4. **Phase 3 — async messaging** (kafkajs/SQS/BullMQ topic matching).
5. **Phase 4 — gRPC checking, tRPC index-only.**

Order-of-magnitude effort (solo, including carrick-cloud halves): Phase 0 ≈ 1
week; Phase 1 ≈ 3–4 weeks; Phase 2 ≈ 2–3 weeks; Phase 3 ≈ 2 weeks. These are
calibration anchors, not commitments.

## 9. MVP brittleness guardrails

The expansion must not lower finding precision below today's REST baseline. Rules
that keep it that way:

- **Drift findings only from deterministic evidence** — parsed SDL, `gql` document
  literals, literal Socket.IO event names, literal queue/topic strings.
  LLM-inferred protocol facts go into the index (queryable over MCP) but never into
  PR-comment drift findings. Extraction misses become silent coverage gaps, not
  false positives.
- **No raw-`ws` drift detection in MVP.** Raw WebSocket frames have no
  protocol-level operation key; anything reported there is a guess. Index-only at
  most.
- **Orphan findings for new protocols default to informational** — unscanned
  consumers (mobile apps, third parties) make "provided but never invoked" a soft
  signal, same as orphaned REST endpoints today.
- **Gateway invisibility argues *for* the new protocols, not against.** REST
  matching is fragile precisely because gateways rewrite the operation key
  (paths/base URLs) — hence `UrlNormalizer` and env-var classification. GraphQL
  operation names, socket event names, and queue topics pass through
  infrastructure untouched, so their matchers have strictly fewer failure modes
  than the REST matcher already shipped.
- **GraphQL checking has no LLM in the loop** (parse + spec-defined validation),
  unlike the REST chain (LLM call-site extraction → sidecar inference → TS
  assignability). Where it fires, it's right.

## 10. LLM pipeline: protocol-routed prompts, not one diluted prompt

**Status: scanner side implemented.** `CandidateTarget` carries a `Protocol`
tag, the orchestrator partitions per file (HTTP prompt sees only HTTP
candidates; unrouted-protocol files are skipped with a stat), and guidance is
a per-protocol map (`ProtocolGuidance`) with `protocol` sent on
`/framework-guidance` requests. The carrick-cloud half (accepting the
protocol field; per-protocol prompt files) goes in the lockstep batch.

A single analyze-file prompt asked to extract every protocol degrades on two
axes: instruction dilution (more rules competing for attention per token of
code) and **response-schema dilution** (every protocol's fields present on
every call invites hallucinated socket events in pure Express files). The
cost would land on the common case — most files are single-protocol REST.
Instead, route:

- **Detection stays generic.** The existing framework-detection inventory
  implies the active protocol set per repo (`express` → http, `socket.io` →
  websocket, `kafkajs` → queue). It is an inventory task, not
  precision-critical: a false protocol only costs one extra guidance call.
- **Guidance becomes per-protocol.** One focused guidance prompt per active
  protocol in carrick-cloud (`system_prompt_http.txt`,
  `system_prompt_websocket.txt`, …); `cached_guidance` becomes a map keyed
  by protocol. Undetected protocols cost nothing.
- **Protocol-tagged SWC candidates are the dispatcher.** The candidate gate
  already decides whether a file reaches the LLM; tagging candidates by
  protocol decides *which prompt* it reaches. Files with only http
  candidates get today's HTTP prompt byte-for-byte unchanged — REST
  extraction precision cannot regress. Mixed files (rare) get one pass per
  protocol with that protocol's prompt and schema; the per-file cache keys
  on `(file, protocol)`.
- **Each pass returns its own result type** (`endpoints`/`data_calls` for
  http, `listeners`/`emits` for sockets), validated by the same generic
  machinery — candidate-ID gating, span attachment, symbol scrubbing are
  protocol-agnostic — then mapped into `OperationKey`s.

Adding a protocol to the LLM layer is then exactly three artifacts: candidate
matchers in the SWC scanner, one prompt + one response schema (carrick-cloud
+ `agents/schemas.rs` in lockstep), and the key mapping. Everything else —
orchestration loop, validation, sidecar, index — is shared.

Standing rule, sharpened by the GraphQL phase (which needed no prompt at
all): a protocol enters the LLM pipeline only where its evidence is not
deterministic. Socket event names and queue topics are literal strings most
of the time — SWC extraction is the default path; the protocol prompt is the
fallback, and its output feeds the index, never drift findings.

## 11. Open questions

- **Index key design in carrick-cloud**: production index keys on
  (workspace, project, repo[, service]); operations need protocol in their identity
  or GraphQL ops named like REST paths could collide. No migration needed — just
  redefine the key and rebuild.
- **Payload growth**: `CloudRepoData` already gates at 5 MB (Lambda limit) by
  dropping caches; adding operation classes makes overflow more likely — may force
  the chunked-upload path sooner.
- **Dynamic keys**: `socket.emit(EVENTS.USER_CREATED, ...)`, topic names from
  constants/env — needs const-resolution similar to what URL extraction already does,
  plus the same env-var classification escape hatch.
- **Which schema wins for GraphQL attribution** when domains aren't configured —
  validate-against-all with best-match reporting is the proposed default; needs
  validation against a real multi-graph org.
