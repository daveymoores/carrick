# named-dispatch

A call proven to send nothing states no call row, whoever stated it
(carrick#2281).

One service:

- `src/registry.ts` stores handlers under names in a `new Map()` the module
  builds and never replaces (`register`, `registerNamed`, `registerAsync`),
  and `dispatch` calls the stored handler. Its `table.get(name)` raises a
  candidate, which makes the module a wrapper module, so a file that imports
  it is asked about even when it raises no candidate of its own.
- `src/remote.ts` hands out callables: `bind` binds a name into a function
  that sends later through a client the scan does not read, `endpoint` does
  the same with a route, and `eager` also sends in its own body.
- `src/reassigned.ts`, `src/own-map.ts` and `src/foreign-map.ts` store into a
  map the module cannot vouch for: a binding assigned again, a `Map` the file
  declares, a `Map` the file imports.
- `src/handlers.ts` and `src/bound.ts` call them at module level, and
  `src/use.ts` calls them inside functions.

The cassette under `__llm__/` states a `GET` at every call under test. A line
under test raises no candidate unless stated, so its row keeps the `@line:N`
id it was written with, joins nothing, and has no span: it is placed by its
line.

Withdrawn: `handlers.ts` lines 16, 17 and 18 (the opening line of a call over
three lines), `bound.ts` line 7, and `use.ts` line 8 (an awaited call with a
string first argument raises a candidate, so that row has a span).

Kept as stated: `handlers.ts` line 20 (an inner line of the three-line call),
22 (the row's text writes the argument's call), 23 (a chain), 24 to 27 (maps
the module cannot vouch for, and a call nothing resolves); `bound.ts` lines 8
(a route written into a callable that may send) and 9 (a factory that sends);
`use.ts` lines 12 (the dispatcher calls the handler) and 16 (a route written
into a callable, inside a function).
