# `class-client-modules`

Fixture for carrick#2452 slice 2: a class client whose fields are set from
constructor parameters, constructed in other modules of the same service.
The compiler's row at each `new` names the class it builds, so every
construction in the service is known. A field reads as the value they all
pass when they agree; anything else keeps the field's own name.

`repo/` is one root service (`carrick .`, `carrick.json` is `{}`). The
store's label for `fetch` in the language's DOM library is the cassette
`repo/__llm__/library-claims/default.json`. Read by
`tests/class_client_modules_test.rs` (the scan) and the unit tests in
`src/request_summary/constructions.rs`.

| Case | Class | Constructions | Expected |
|---|---|---|---|
| 1 | `src/api.ts` `Api`: a parameter property `base` | `new Api("/api/v1")` in `src/api-instance.ts`; called from `src/use-api.ts` | `GET /api/v1/users` at `src/use-api.ts:4`, the call into the method |
| 2 | `src/items.ts` `Items`: `this.send = opts.send ?? fetch`, `this.base = opts.base` | `new Items({ base: "/v2" })` in `src/root.ts` | `GET /v2/items` at `src/items.ts:18`: no `send` is passed, so the client sends with `fetch` |
| 3 | `src/region.ts` `Region` | `new Region("/east")` and `new Region("/west")` in two modules | no row states `/east/ping` or `/west/ping`: the field keeps its name |
| 4 | `src/feed.ts` `Feed` | `new Feed("/feed")`, and `LiveFeed extends Feed` in `src/live-feed.ts` | no row states `/feed/recent`: a subclass may pass anything |
| 5 | `src/reports.ts` `Reports` | `new Reports(base)` inside `reportsAt(base)` | the field keeps its name: no text here states what callers pass |
| 6 | `src/jobs.ts` `Jobs`: `this.send = options.send ?? fetch`, `options = {}` | `new Jobs(options)` inside `jobsFrom(options = {})`, which nothing calls | `POST /api/jobs` at `src/jobs.ts:17`: the factory's default holds no `send` |

Case 5 and case 6 differ on purpose. Text a field holds is read only from
what a construction writes. A function a field holds is read the way a
callee that is a value is (carrick#2432): a construction that hands on its
own parameter is followed to its own callers, and with none, to the
parameter's default.
