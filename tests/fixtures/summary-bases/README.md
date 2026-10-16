# `summary-bases`

Fixture for carrick#2406 slice B: the base of a row the request summaries
state at a call to a function another module declares is read from where the
base piece ends (an environment read, a field, a parameter, an import, a
global), not from the target's spelling.

Every model answer is empty, so each row is the summaries' own.

## The answer key

| site | shape | base |
|---|---|---|
| `src/use.ts:6` | `client.list()`, a class field `${process.env.SVC_URL ?? "…"}/v1` | `env`, `SVC_URL` |
| `src/use.ts:7` | `paramClient.items()`, a field a constructor parameter fills | `injected` |
| `src/use.ts:8` | `movingClient.items()`, a field written twice | `injected` |
| `src/use.ts:9` | `getThings()`, an alias `things.ts` imports and `use.ts` does not | `injected` |
| `src/use.ts:10` | `ping()`, `${API_ORIGIN}` that no file declares or imports | `injected` (a global; the removed SCREAMING_SNAKE guess said `env`) |
| `src/rewritten.ts:5` | `getThings()` in a file that imports the alias, so its slot is rewritten to `${process.env.API_BASE_URL}` | `env`, `API_BASE_URL` |

`SVC_URL` and `API_BASE_URL` are undeclared on purpose: an undeclared env-var
base is an expected find.
