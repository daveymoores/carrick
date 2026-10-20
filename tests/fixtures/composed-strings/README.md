# `composed-strings`

Fixture for carrick#2447: a request URL built from parts whose values the
source states, read as one value. Invented for the shape; no file here is
copied from a scanned project.

## The shape

A client funnels its requests through one function and writes each URL as a
base joined to a path:

```ts
const apiFetch = (url: string) => fetch(url);
apiFetch(getApiBase() + "/session");   // getApiBase() returns "/api/v1"
apiFetch(`${PREFIX}/x/${id}`);         // PREFIX = "/api/v1"
```

The base is declared in `src/config.ts`, so the client reads it through an
import. Before carrick#2447 an imported builder call, an entry of an imported
constant object and a function with a local before its `return` were each an
unread value, and the row kept the unread base in its target.

## The answer key

`src/client.ts` (the base is imported from `src/config.ts`):

| line | the URL is | the row's target |
|---|---|---|
| 17 | `getApiBase() + "/session"` | `/api/v1/session` |
| 20 | `` `${PREFIX}/x/${id}` `` | `/api/v1/x/${id}` |
| 23 | `sessionBase() + "/local"`, the builder has a local `const` before its `return` | `/api/v1/local` |
| 26 | `PATHS.users + "/m"`, an entry of an `as const` object | `/users/m` |
| 29 | `pick(flag) + "/two"`, the builder returns `"/a"` on one path and `"/b"` on the other | no row: the call reads as `/a` or `/b`, so no one path is its (carrick#2541) |
| 32 | `same(flag) + "/same"`, every path returns `"/a"` | `/a/same` |
| 35 | `process.env.X + "/env"` | `${process.env.X}/env`: a runtime value |
| 41 | `MUTABLE.users + "/mutable"`, another function writes `MUTABLE.users` | `${MUTABLE.users}/mutable` |
| 44 | `viaLet() + "/let"`, the builder holds its value in a `let` | `${viaLet()}/let` |
| 47 | `afterLoop([]) + "/loop"`, the builder runs a loop before its `return` | `${afterLoop([])}/loop` |
| 50 | `itemPath("abc") + "/text"`, the builder is handed text | `/items/abc/text` |
| 53 | `itemPath(id) + "/param"`, the builder is handed a parameter | `${itemPath(id)}/param` |

`src/local.ts` repeats the builder shapes with the builders declared in the
same file as the client, which is read without any import:

| line | the URL is | the row's target |
|---|---|---|
| 21 | `base() + "/session"` | `/api/v1/session` |
| 24 | `sessionBase() + "/local"` | `/api/v1/local` |
| 27 | `pick(flag) + "/two"` | no row: the call reads as `/a` or `/b`, so no one path is its (carrick#2541) |
| 30 | `same(flag) + "/same"` | `/a/same` |

## The cassettes

Every cassette is empty: the only candidate each file raises is the
`apiFetch` wrapper's own `fetch(url)`, whose target is a parameter with one
definition per caller. So every row is read off the source by the request
summaries.
