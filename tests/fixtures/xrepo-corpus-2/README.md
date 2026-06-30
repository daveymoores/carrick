# xrepo-corpus-2 — authored event-driven cross-repo accuracy corpus

Five statically-analyzable (not runnable) repos with an owned answer key, for the
cross-repo accuracy eval. The deliberate **dual of corpus-1**: where corpus-1 is a
request/response topology, corpus-2 is **event-driven**, centered on a new
**pub/sub family** protocol (Kafka + Redis pub/sub + NATS) with typed payloads, so
each broker carries a real cross-repo type-compat edge. Labels are
**spec-of-record**: authored first, code built to match, never derived from a scan.
Change a label only by hand, in a separate commit, reviewed against the spec.

Selected via `CARRICK_EVAL_CORPUS=xrepo-corpus-2` through the same two-phase scorer
(`tests/eval_xrepo.rs`); default (`xrepo-corpus-1`) is unaffected.

## Why this corpus exists
1. **Validate the pub/sub family** as ONE topic-keyed abstraction across three
   brokers from the start (anti-overfit: no single-broker hack). Subscriber = the
   contract **producer** (endpoint); publisher = the **consumer** (call). Cross-repo
   key is `pubsub|<topic>` — the broker is detection metadata, **not** part of
   identity (a publisher and subscriber on the same topic string match regardless of
   broker; corpus-2 gives each topic a single broker, so the cross-broker case is a
   future-corpus + unit-test concern, not exercised here).
2. **Stress generalization of existing scanner features** with mechanisms tagged
   `roadmap` (scored separately, see Tiers): code-first GraphQL (Pothos),
   TypedDocumentNode / graphql-ws consumers (no `gql` tag), a real GraphQL
   subscription (async-generator resolver), async iterators.
3. **Framework-agnostic proof** via three HTTP frameworks: Fastify, Hono, Express
   (alongside corpus-1's Express/NestJS), tagged `capability`.

## Repos
- `notifications-svc/` (Fastify + kafkajs + nats.js) — pub/sub **subscriber** (=
  producer) of Kafka `order.placed` and NATS `user.registered` (async iterator),
  plus a Fastify HTTP producer (`GET /notifications/:id`, `GET /health` orphan).
- `orders-engine/` (NestJS + Pothos + kafkajs) — Kafka `order.placed` **publisher**
  (= consumer; payload wrapped in `Envelope<T>`), and a **code-first Pothos** schema:
  `query order`, `mutation cancelOrder` (orphan), `subscription orderEvents`
  (async-generator). Hosts the `__dlq.retry` intra-repo self-loop (see Decoys).
- `analytics-worker/` (Hono + ioredis + nats.js) — **mixed-broker publisher**: Redis
  `metrics.page_view` and NATS `user.registered`; Hono HTTP producer (`POST /track`).
- `web-dashboard/` (Next-ish + @graphql-typed-document-node/core + graphql-ws +
  ioredis) — GraphQL **consumer** via TypedDocumentNode (`query order`) and graphql-ws
  (`subscription orderEvents`); Redis `metrics.page_view` **subscriber** (= producer);
  HTTP consumer of `/notifications/:id` + `/track`.
- `billing-svc/` (Express + kafkajs + nats.js) — Kafka `order.placed` publisher with
  an **incompatible** payload; NATS `payment.captured` publisher (orphan); HTTP
  consumer of `${LEDGER_URL}/ledger/append` (orphan).

## Configuration
Consumer repos with `${ENV}`-based **HTTP** calls carry a `carrick.json` with
`internalEnvVars` (web-dashboard: `NOTIFICATIONS_URL`, `ANALYTICS_URL`; billing-svc:
`LEDGER_URL`) — the same internal-classification gate as corpus-1 (without it the
HTTP edges never form). **Pub/sub (like GraphQL/Socket.IO) needs no
connection-classification config**: it keys on the topic literal, not a URL.

## Tiers
`capability` = scored in the headline `overall_correctness`. `roadmap` = tracked but
partitioned out. The 2 code-first-GraphQL edges (Pothos producer + TypedDocumentNode /
graphql-ws consumer) are **`roadmap`**: the scanner is schema-first/SDL+`gql`-tag only
(`src/graphql.rs`), so scoring them `capability` would crater the headline for work the
pub/sub program does not ship. They are next-program headroom, flipped to `capability`
only when their detection lands — never to inflate the score.

## Cross-repo edges
| # | Protocol | Producer (subscriber) | Consumer (publisher) | Key | Compat | Tier |
|---|---|---|---|---|---|---|
| 1 | pubsub/kafka | notifications-svc `order.placed` | orders-engine | `pubsub\|order.placed` | compatible | capability |
| 2 | pubsub/kafka | notifications-svc `order.placed` | billing-svc | `pubsub\|order.placed` | **incompatible** (`total` `number` vs `{amountCents,currency}`) | capability |
| 3 | pubsub/nats | notifications-svc `user.registered` | analytics-worker | `pubsub\|user.registered` | compatible | capability |
| 4 | pubsub/redis | web-dashboard `metrics.page_view` | analytics-worker | `pubsub\|metrics.page_view` | compatible | capability |
| 5 | graphql | orders-engine `query order` | web-dashboard | `graphql\|query\|order` | compatible | roadmap |
| 6 | graphql | orders-engine `subscription orderEvents` | web-dashboard | `graphql\|subscription\|orderEvents` | **incompatible** (consumer adds `"cancelled"` union member) | roadmap |
| 7 | http | notifications-svc `GET /notifications/:id` | web-dashboard | `http\|GET\|/notifications/:param` | compatible | capability |
| 8 | http | analytics-worker `POST /track` | web-dashboard | `http\|POST\|/track` | compatible | capability |

**Pub/sub producer/consumer direction (read before "fixing" the labels).** Mirroring
the socket model: a *subscriber* (`consumer.subscribe` / `sub.on('message')` /
`nc.subscribe`) is the **producer (endpoint)** of the topic it receives; a *publisher*
(`producer.send` / `redis.publish` / `nc.publish`) is the **consumer (call)**. So for
`order.placed` (orders-engine + billing-svc *publish* → notifications-svc *subscribes*),
the `matches` edge has `producer_repo: notifications-svc` (subscriber) and
`consumer_repo:` the publisher. The *event* flows publisher → subscriber; the *contract
producer* is the subscriber.

Orphan producers: orders-engine `graphql mutation cancelOrder` (roadmap),
notifications-svc `http GET /health`. Orphan consumers: billing-svc
`pubsub payment.captured`, billing-svc `http POST /ledger/append`.

`dependency_conflicts` carries one deliberate cross-repo conflict: `ioredis` 4.28.0
(web-dashboard) vs 5.3.0 (analytics-worker) — major-incompatible → `critical`,
exercising the semver-major gate. Kept in-model (a pub/sub dep).

## Decoys (anti-overfit traps that must extract nothing as a cross-repo edge)
1. `analytics-worker/src/redis/publisher.ts` — `redis.set('metrics.page_view', …)` /
   `redis.get(…)`: ioredis **key-value cache** on a string that happens to equal a
   topic. NOT pub/sub. The "don't hallucinate a topic from a cache key" trap.
2. `orders-engine/src/kafka/dlq.ts` — `__dlq.retry` is **published AND subscribed
   within the same repo** (dead-letter self-loop). It is real pub/sub but an
   intra-repo self-edge; it must NOT surface as a cross-repo match.
3. `billing-svc/src/http/client.ts` — `kafka.admin().createTopics(…)`: topic
   **administration**, not publish/subscribe.

### Decoy-scoring limitation (logged, not silent)
`score_decoy_leak` (`tests/eval_xrepo.rs`) currently matches `_must_not_emit` entries
only against HTTP `(method, path)` projections; it skips non-HTTP ops. The three
pub/sub decoys above are therefore **documented but not yet counted** by the decoy
metric, and decoys 1/2 additionally collide on topic-key with a legit op on the same
topic (so set-precision can't distinguish them either). Extending decoy scoring to
non-HTTP ops (source-location-keyed, to survive the topic-key collision) is a deliberate
follow-up of the pub/sub slice; until then these traps are validated only by manual
projection inspection. Pre-slice the scanner cannot emit pub/sub ops at all, so the
decoy count is trivially 0.

## Phasing (what scores when — do not mistake a low pre-slice run for a corpus bug)
- **Pass immediately** (existing machinery, `capability`): the `ioredis` dep conflict;
  the 2 HTTP edges (Fastify/Hono — Hono is medium-confidence, real framework-agnostic
  headroom if it misses); the HTTP orphans.
- **Start near 0 → climb as the pub/sub slice + extraction land**: all 4 pub/sub edges
  (ep/call/match/anchor/resolution), then pub/sub compat **last** (needs the ts_check
  direction + codec-unwrap path; until then pub/sub matches return `None` and the
  compat dimension reads as absent, exactly the corpus-1 graphql/socket arc).
- **`roadmap`, partitioned out**: the 2 code-first-GraphQL edges (Pothos detection +
  TypedDocumentNode/graphql-ws consumer are detection cliffs).

## Ground truth
- Per-repo `<repo>/expected.json` — HTTP `endpoints`/`calls` + the non-HTTP
  `graphql_operations` and additive `pubsub_operations` arrays (each op carries
  `role`/`key`/`primary_type_symbol`/`resolved_type`/`type_state`/`tier`; for pub/sub
  the `topic`/`broker`/`side`/`source` fields are diagnostic and ignored by the scorer).
  `_must_not_emit` lists the decoys.
- Corpus `expected-output.json` — cross-repo `matches` (with `protocol`/`tier`/compat),
  `orphans`, `dependency_conflicts`, and a `summary`.
