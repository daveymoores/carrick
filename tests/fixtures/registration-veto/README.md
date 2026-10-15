# registration-veto

A route registration that takes a path, an options bag and a handler
(carrick#2383), next to a request that takes the same three arguments.

| File | Line | Shape | Rows |
|---|---|---|---|
| `src/setup.ts` | 6, 7 | `router.define(path, { method, body }, handler)` from a package: the source states an endpoint at each call | endpoints `POST /reset`, `DELETE /purge`; no request row here, and none at the call of `setup()` in `src/app.ts` |
| `src/check.ts` | 4 | `request(path, { method }, (res) => ...)`: nothing states a route there | `GET /status` at the call of `check()` in `src/app.ts` line 6 |

The model's cassette (`__llm__/analyze-file/setup.json`) states the two
endpoints. `src/app.ts` is the caller of both functions.
