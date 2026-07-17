# pubsub-wrapper-monorepo — generic wrapper-pattern pub/sub fixture

An owned, dependency-free monorepo fixture for the pub/sub expression-locator
path: three wrapper shapes whose payload types are **never a bare named symbol
at any publish/subscribe site** — the type is carried by a generic binding in
the shared `@fixture/contracts` workspace package:

1. **Topic-map bus** (`TopicBus<Events>`): a typed event emitter over a
   topic → payload-tuple map. Publisher emits an inline object literal;
   subscriber destructures an unannotated handler param.
2. **Schema-catalog worker** (`QueueWorker<Catalog>`): payload types derived
   from value-level schemas via mapped/conditional types (`InferSchema`), the
   way schema libraries do. Publisher's payload is a `payload:` property
   initializer; subscriber's is a binding element inside the job envelope.
3. **Channel-handle factory** (`channel<T>({ id })`): the payload type is a
   declaration-site type argument; the pub/sub sites carry nothing.

`carrick.json` splits the repo into two services (`dispatch` = publishers,
`relay` = subscribers) so the topics form real cross-service edges rather than
intra-repo self-loops (which the projection deliberately drops).

The `__llm__/` cassettes replay the file-analyzer output offline, including
the `payload_expression_text` / `payload_expression_line` locator fields; the
sidecar resolves them deterministically with tsc (`expression` infer for
publishers, `function_param` for subscribers). Driven by
`tests/pubsub_wrapper_type_capture_test.rs`.

Pre-fix baseline (why this fixture exists): all six ops extracted and matched,
but `type_state=Unknown` on both sides of every edge — the named-symbol anchor
(`primary_type_symbol`) is structurally unable to capture generically-bound
payloads, and the schema correctly instructs null for them.

Cassette line numbers reference exact source lines in `services/*/src/*.ts`;
re-count them after any edit to those files.
