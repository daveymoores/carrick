# xrepo-corpus-1 — authored cross-repo accuracy corpus

Three statically-analyzable (not runnable) repos with an owned answer key, for
the cross-repo accuracy eval (epic #207). Labels are **spec-of-record**: authored
first, code built to match, never derived from a scan. Change a label only by hand,
in a separate commit, reviewed against the spec — never to match scanner output.

- Field shapes and per-metric comparison rules: the scorer contract
  (`carrick-cloud/docs/internal/cross-repo-eval-scorer-contract.md`).
- Per-failure-mode spec: build plan §6.

## Repos
- `orders-monorepo/` — npm-workspaces producer side. `orders-pkg` (Fastify):
  renamed scoped-closure plugin param (owner-drift trap, #133/#167),
  `new Router({prefix})` mounted with no path, barrel re-export. `gateway`
  (NestJS): `@Controller('users')`+`@Get(':id')` prefix concat, an owner=method
  fabrication trap (`/gateway/health`), an MCP `server.tool()` decoy, an
  **Implicit-return** `@Get('recent')` (no annotation → inferred shape, #222),
  and a **schema-first GraphQL** producer (`src/schema.graphql` +
  `src/orders.resolver.ts`, #220).
- `payments-svc/` — Express producer + consumers: env-var-base `/orders/:param`
  (compatible edge), orphan `/billing/charge`, `sendBeacon('/metrics/ingest')`
  built-in (must emit, `import_source: null`), SDK-as-HTTP decoy (`audit.ts`),
  Lambda-handler decoy (`settle.ts`), and a **Socket.IO server emit** of
  `payment:settled` (`realtime/server.ts`, #221).
- `web-frontend/` — Next.js-style consumer: REST `/orders/[id]` (type-INCOMPATIBLE
  with the producer) and `/payments` (compatible); a **GraphQL client**
  (`lib/graphql.ts`: `query order` compatible, `subscription orderUpdated`
  INCOMPATIBLE via optional-field widening, #220/#222); a **Socket.IO client
  listener** for `payment:settled` (`lib/realtime.ts`, #221).

## Configuration
The consumer repos carry a `carrick.json` declaring their env-var base URLs as
`internalEnvVars`. Cross-repo matching of an `${ENV}`-based **HTTP** call to a
producer only fires when the env var is classified internal
(`Config::is_internal_call`, matcher at `src/analyzer/mod.rs`), so without this
the HTTP edges never form (a fresh corpus scan showed cross-repo match F1 = 0).
This is deliberate: the explicit internal/external classification is kept
(internal-by-default was considered and rejected — see
`carrick-cloud/docs/internal/cross-repo-call-classification-decision.md`).

**GraphQL and Socket.IO need no connection-classification config.** Their
matchers (`Analyzer::analyze_exact_key_matches`, `src/analyzer/mod.rs`) key on
the *operation identity*, not the URL: GraphQL on `OperationKey::Graphql { kind,
field }` (e.g. `graphql|query|order`) and Socket.IO on `OperationKey::Socket {
event, direction }` (e.g. `socket|SERVER->CLIENT|payment:settled`). A consumer
document field meets a producer schema field by name; a socket emitter meets a
listener on the same event+direction — regardless of which URL either side
connects to. So the GraphQL/socket connection URLs
(`NEXT_PUBLIC_GATEWAY_GQL_URL`, `NEXT_PUBLIC_PAYMENTS_WS_URL`) are left out of
`internalEnvVars` on purpose; adding them would be inert for matching.

## Ground truth
- Per-repo `<repo>/expected.json` — HTTP `endpoints`/`calls` + owner + type
  anchor + resolved type + `_must_not_emit` negatives, plus the non-HTTP
  `graphql_operations` and `socket_events` arrays (each with `role`/`service`/
  `key`/`primary_type_symbol`/`resolved_type`/`type_state`). Every label tagged
  `capability`/`roadmap` (all `capability` here). The current S4-thin-slice
  scorer (`tests/eval_xrepo.rs`) reads only `endpoints[].{method,path}` and
  ignores the new arrays via serde defaults; the full S4 scorer (#223) scores
  them.
- Corpus `expected-output.json` — cross-repo `matches` (now carrying a
  `protocol` tag per edge + the compat verdict), `orphans`, and
  `dependency_conflicts`.

## Cross-repo edges
| Protocol | Producer | Consumer | Key | Compat |
|---|---|---|---|---|
| http | orders-pkg `GET /orders/:param` | payments-svc | `http\|GET\|/orders/:param` | compatible |
| http | orders-pkg `GET /orders/:param` | web-frontend | `http\|GET\|/orders/:param` | **incompatible** (`id` string vs number) |
| http | payments-svc `POST /payments` | web-frontend | `http\|POST\|/payments` | compatible |
| graphql | gateway `query order` | web-frontend | `graphql\|query\|order` | compatible |
| graphql | gateway `subscription orderUpdated` | web-frontend | `graphql\|subscription\|orderUpdated` | **incompatible** (optional-field widening: consumer `note` required, producer optional) |
| socket | web-frontend (listener) | payments-svc (emitter) | `socket\|SERVER->CLIENT\|payment:settled` | compatible |

**Socket producer/consumer direction (read before "fixing" the labels).** In
Carrick's socket model a *listener* (`socket.on`) is the **producer (endpoint)**
of the key it receives and an *emitter* (`socket.emit`) is the **consumer
(call)** of the key it sends (`src/socket_io.rs`, `src/engine/mod.rs`: "listeners
as endpoints, emitters as calls"; unit test `test_socket_matching_is_direction_aware`).
So for the `payment:settled` event flow (payments-svc server *emits* →
web-frontend client *listens*), the structured `matches` edge has
`producer_repo: web-frontend` (the listener) and `consumer_repo: payments-svc`
(the emitter), both on `socket|SERVER->CLIENT|payment:settled`. The *event*
flows payments-svc → web-frontend; the *contract producer* is the listener.

Orphan producers: orders `GET /api/v1/status`, gateway `GET /users/:param`,
`GET /users/recent`, `GET /gateway/health`, gateway `graphql query orders`,
gateway `graphql mutation refundOrder`; payments `GET /payments/:param`. Orphan
consumers: payments `POST /billing/charge`, `POST /metrics/ingest`. Decoys (MCP
tools, `audit.ts`, `settle.ts`) emit nothing.

`dependency_conflicts` carries one deliberate cross-repo conflict (`zod` 3.x vs
4.x). Its exact version-string and severity are deterministic scanner output,
verified against a real scan when the scorer (S4) / cassette gate (S5) wire the
corpus in.

## Type-inference cases (#222)
The corpus previously had `type_state` 9×Explicit / 2×Unknown / **0×Implicit**
and only flat named interfaces. Threaded through the new edges:
- **Implicit (inferred) return type** — gateway `GET /users/recent` has no
  return annotation; the sidecar must infer `{ count: number; ids: string[] }`
  (`type_state: Implicit`, the corpus's first and only one).
- **Generic wrapper** — `ApiResponse<T>` wraps the GraphQL `order`/`refundOrder`
  payloads (`src/orders.resolver.ts`).
- **Nested object** — `Money` (`{ amountCents; currency }`) inside `Order.total`.
- **Optional field** — `Order.note?` on the producer.
- **Union field** — `OrderStatus` (`placed | refunded`), modelled as a GraphQL
  `union OrderStatus` and a discriminated TS union.
- **Subtler compat mismatch** — the `orderUpdated` subscription widens the
  optional producer `note` into a required consumer `OrderUpdate.note`, a quieter
  mismatch than the blunt `id: number` vs `string` REST trap.

## Protocol detectability (what the scanner keys on)
- **GraphQL producer**: `gateway/src/schema.graphql` (SDL). `scan_repo`
  (`src/graphql.rs`) walks the repo for `.graphql`/`.gql` files and indexes
  root-operation fields (`Query`/`Mutation`/`Subscription`) as producers.
- **GraphQL consumer**: `gql\`…\`` tagged templates in `web-frontend/lib/graphql.ts`
  (`extract_from_ts_file`); the top-level executable-document field is the
  consumer key (alias-aware, `__typename`/introspection skipped).
- **Socket producer/consumer**: `new Server()` from `socket.io` +
  `io.on('connection', socket => socket.emit('payment:settled', …))`
  (payments-svc emitter) and `io(url)` from `socket.io-client` +
  `socket.on('payment:settled', …)` (web-frontend listener). String-literal
  event names only; `connect`/`disconnect` reserved events are filtered.
