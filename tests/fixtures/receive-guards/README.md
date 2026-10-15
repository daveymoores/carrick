# Receive-side guard fixtures (carrick#2386)

Each `<name>.guards.json` is the hand-written answer key for one fixture: every
chain the guard summaries (`src/request_summary/guards.rs`) state for a
route-shaped literal the code compares a request value against. The keys were
written from the source before the pass existed. Never edit a key to make a
test pass.

`hand-dispatch-worker.guards.json` keys the existing
`tests/fixtures/hand-dispatch-worker/`; the others key the directory of the
same name here.

- `worker-variants/`: `export default { … } satisfies T`, a `const` object
  exported as the default, and an in-repo caller that hands the handler a
  literal request.
- `node-server/`: a handler passed by name and one passed inline to a
  package's server factory, a `switch`, and an object-literal table.
- `dev-middleware/`: a middleware registered through a parameter of a plugin
  hook, inside a config object handed to a package export.
- `predicate-elsewhere/`: predicates in other files, reached by a relative
  import, a `.js` specifier and a tsconfig alias, one of them negated, one
  through two calls.

Slice B2 (carrick#2386) widens the guards to every string literal and to
constants read through imports. Each chain then states a `shape`: `path` for a
route-shaped literal, `name` for any other, and `unresolved` for a constant the
value flow cannot read. The five keys above hold only the `path` chains, and
their tests compare only those, with `shape` left out. The keys below hold every
chain.

- `message-switch/`: a child process's `message` listener that switches on a
  message field, one reached through a parse call, and a worker that compares
  with a constant object imported from a sibling file and with one from a
  package.
- `constant-routes/`: a `node:http` server that compares the path with a
  constant imported through a re-export and with one from a package, and a
  route registered under an imported constant with spread handlers, which
  states no guard.
- `handler-tables/`: a module-scope handler table with name keys, indexed
  with a message field; a handler table handed to a package's constructor;
  and a `Map` filled with `.set(Ops.SAVE, fn)`, where `Ops` is a `const`
  object in a sibling file.

Slice B2b (carrick#2386) adds the calls that name an operation in their
arguments: a registration (a name and a function), an emission (a name and no
function) and a construction (`new X(…)` with a name). Each `<name>.calls.json`
holds every such call one fixture makes, with its names, its functions, the
chain its callee is followed down, and the package receiver the library-site
reader places it on, where it places one.

- `listeners-and-sends/`: a class whose field is a runtime emitter, emitting
  in one file and listened to in another; a listener on a package import; a
  listener on a parameter with one in-repo caller; and a send on a package
  maker's instance.
- `message-switch/`, `constant-routes/` and `handler-tables/` also hold a
  `.calls.json` key: the listeners, the sends, and a route registered under
  an imported constant with spread handlers.

Lines are 1-based. Files are relative to the fixture root.
