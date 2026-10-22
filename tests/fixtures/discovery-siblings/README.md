# Discovery across services fixture (carrick#2618)

`repo/` has two services of one run and one package that belongs to none:

| Path | Service | Role |
|---|---|---|
| `services/app` | `app` | Its `tsconfig.json` loads `es2022` only (no DOM): it never calls `fetch` itself |
| `packages/client` | `client` | Sends through a local helper that calls `fetch`; its `tsconfig.json` loads `dom` |
| `packages/loose` | none | `ping()` calls `fetch`, but no service reads the file |

`library-claims.json` is the store's answer for the default library's
`fetch` (`lib.dom`).

## Answer key

Discovery of `app` with `client` as its sibling
(`discover_with`, `src/request_summary/discovery.rs`), for `services/app/src/main.ts`:

| Line | Call | Record |
|---|---|---|
| 5 | `getItem(id)` | A site. End: `fetch` in `packages/client/src/http.ts`. Chain: `getItem`, `send` |
| 9 | `postTo(…)` | A site. Chain: `postTo`, `send` |
| 14 | `items.remove(id)` | A site. Chain: `ItemsClient.remove`, `send`. No record at `new ItemsClient()` |
| 18 | `format(n)` | No record |
| 22 | `ping()` | `unfollowed`, `outside_service`: its file is in no service |

No record, site or unfollowed, names a file under `packages/`: a run records
only the calls of its own service's files. Without siblings, lines 5, 9 and
14 are `unfollowed: outside_service`.
