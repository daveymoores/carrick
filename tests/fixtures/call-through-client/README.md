# call-through-client

Model rows at calls through the repository's own code (carrick#1146).

Four services:

- `packages/client` writes the requests: seven as request specs
  (`transport.get({ url: "/api/v1/runs/{id}" })`), one as a `fetch` with a
  literal path, and one whose whole URL is its parameters.
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
46, 101, 133 and 139; `hooks.ts` lines 9, 14 and 19) carry the reached
operation, the source `request_summary`, the role `wrapper_call` and
`reaches_request`. Line 133 calls the client method on the factory's result
inline (`getClient().getRun(id)`), and line 139 calls that command. Line
21 is stated twice by the model and is one row afterwards. The correct row
(`commands.ts:28`) keeps its `model` source and its type entry, and gains the
role and the link.

Types at the corrected rows:

| Site | The type anchored there |
|---|---|
| `commands.ts:9` | `Run`, which matches what the API sends: kept under the reached operation and judged compatible |
| `commands.ts:46` | `SavedRun`, a stale copy that requires a field the API no longer sends: kept and judged incompatible |
| `hooks.ts:9`, `:14`, `:19` | anchored at a hook's or a runner's call, whose result is not what the request returns: dropped and counted |

No corrected row carries a request type: a call's arguments are the called
function's parameters, not a request body.

Rows that stay as the model stated them:

| Site | Why |
|---|---|
| `commands.ts:57` | the helper called reaches two operations |
| `commands.ts:64` | every method of that client also requests the health route |
| `commands.ts:70` | nothing resolves the receiver |
| `commands.ts:76` | the function called writes its URL from its parameters, so no row states its request |
| `commands.ts:82` | the site writes the route |
| `commands.ts:88` | the site also writes a request of its own |
| `commands.ts:94` | only the model states the request the function called makes |
| `commands.ts:127` | the site makes a request itself, inside a call that reaches another |
| `hooks.ts:24` | two callbacks, and nothing says which makes the request |

`commands.ts:113` writes its route into a function of the same file; the
scanner reads that row off the source and this pass never touches it.

`commands.ts:101` reaches a request the API serves under another verb, which
is the finding the pull request test reads.

Driven by `tests/call_through_rows_test.rs`.
