# call-through-client

Model rows at calls through the repository's own code (carrick#1146).

Four services:

- `packages/client` writes the requests: seven as request specs
  (`transport.get({ url: "/api/v1/runs/{id}" })`), one as a `fetch` with a
  literal path, one whose whole URL is its parameters, and two as a `fetch`
  on a declared environment base: one hands back its parsed body, the other
  wraps it in an envelope.
- `packages/cli` reaches them through a client class, a factory, and commands
  that call other commands.
- `apps/web` reaches them from hook arguments: a call spread into the hook's
  options, a callback among them, and a callback handed to one of the app's
  own functions.
- `services/api` serves the routes.

The cassette under `__llm__/` states what a model states at a call that writes
neither a method nor a path: a path made from the method's name, a base it
invented, a sibling's route, and one correct path. Rows whose id starts with
`model:` join no candidate, so they have no span and are placed by their line.

Rows that take the request their call reaches (`commands.ts` lines 9, 15, 21,
46, 64, 101, 145, 151, 157, 164 and 176; `hooks.ts` lines 9, 14 and 19) carry the reached operation, the
source `request_summary`, the role `wrapper_call` and `reaches_request`. Line
21 is stated twice by the model and is one row afterwards. The correct row
(`commands.ts:28`) keeps its `model` source and its type entry, and gains the
role and the link.

Types at the corrected rows:

| Site | The type anchored there |
|---|---|
| `commands.ts:145` | `Run`, which matches what the API sends; the function called hands back the parsed body: kept under the reached operation and judged compatible |
| `commands.ts:157` | `SavedRun`, a stale copy that requires a field the API no longer sends: kept and judged incompatible |
| `commands.ts:151` | anchored at a call whose function wraps the body in an envelope: dropped and counted |
| `commands.ts:9`, `:21`, `:46`, `:101` | anchored at a client method that returns `.data` of what a request spec returns, a shape nothing here states: dropped and counted |
| `hooks.ts:9`, `:14`, `:19` | anchored at a hook's or a runner's call, whose result is not what the request returns: dropped and counted |

A type is kept only where the function called hands back the request's parsed
body unchanged, as the request summaries read it (carrick#1601).

Where a call reaches more than one request, the request is the one whose value
the functions hand back. A request awaited and dropped, or whose value only
goes into the arguments of another request, is a side request
(`src/return_flow.rs`):

| Site | Side request | Reached |
|---|---|---|
| `commands.ts:64` | the client's health probe, a request a fact states | `GET /api/v1/runs/:id` |
| `commands.ts:164` | the client's health probe, a `fetch` only the model reads | `GET /api/v1/runs/:id` |
| `commands.ts:176` | the token handed to the request returned | `GET /api/v1/runs/:id` |

No corrected row carries a request type: a call's arguments are the called
function's parameters, not a request body.

Rows that stay as the model stated them:

| Site | Why |
|---|---|
| `commands.ts:57` | the helper called reaches two operations |
| `commands.ts:70` | nothing resolves the receiver |
| `commands.ts:76` | the function called writes its URL from its parameters, so no row states its request |
| `commands.ts:82` | the site writes the route |
| `commands.ts:88` | the site also writes a request of its own |
| `commands.ts:94` | only the model states the request the function called makes |
| `commands.ts:127` | the site makes a request itself, inside a call that reaches another |
| `commands.ts:139` | the function called asks for a token and returns a `fetch` only the model reads |
| `hooks.ts:24` | two callbacks, and nothing says which makes the request |

`commands.ts:113` writes its route into a function of the same file; the
scanner reads that row off the source and this pass never touches it.

`commands.ts:101` reaches a request the API serves under another verb, which
is the finding the pull request test reads.

Driven by `tests/call_through_rows_test.rs`.
