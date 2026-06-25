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
  fabrication trap (`/gateway/health`), and an MCP `server.tool()` decoy.
- `payments-svc/` — Express producer + consumers: env-var-base `/orders/:param`
  (compatible edge), orphan `/billing/charge`, `sendBeacon('/metrics/ingest')`
  built-in (must emit, `import_source: null`), SDK-as-HTTP decoy (`audit.ts`),
  Lambda-handler decoy (`settle.ts`).
- `web-frontend/` — Next.js-style consumer: `/orders/[id]` (type-INCOMPATIBLE
  with the producer) and `/payments` (compatible).

## Configuration
The consumer repos carry a `carrick.json` declaring their env-var base URLs as
`internalEnvVars`. Cross-repo matching of an `${ENV}`-based call to a producer
only fires when the env var is classified internal (`Config::is_internal_call`,
matcher at `analyzer/mod.rs`), so without this the edges never form (a fresh
corpus scan showed cross-repo match F1 = 0). This is deliberate: the explicit
internal/external classification is kept (internal-by-default was considered and
rejected — see `carrick-cloud/docs/internal/cross-repo-call-classification-decision.md`).

## Ground truth
- Per-repo `<repo>/expected.json` — endpoints/calls + owner + type anchor +
  resolved type + `_must_not_emit` negatives, every label tagged
  `capability`/`roadmap` (all `capability` here).
- Corpus `expected-output.json` — cross-repo `matches` (with compat verdict),
  `orphans`, and `dependency_conflicts`.

## Cross-repo edges
| Producer | Consumer | Compat |
|---|---|---|
| orders-monorepo `GET /orders/:param` | payments-svc | compatible |
| orders-monorepo `GET /orders/:param` | web-frontend | **incompatible** (`id` string vs number) |
| payments-svc `POST /payments` | web-frontend | compatible |

Orphan producers: orders `GET /api/v1/status`, gateway `GET /users/:param`,
`GET /gateway/health`, payments `GET /payments/:param`. Orphan consumers:
payments `POST /billing/charge`, `POST /metrics/ingest`. Decoys (MCP tools,
`audit.ts`, `settle.ts`) emit nothing.

`dependency_conflicts` carries one deliberate cross-repo conflict (`zod` 3.x vs
4.x). Its exact version-string and severity are deterministic scanner output,
verified against a real scan when the scorer (S4) / cassette gate (S5) wire the
corpus in.
