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

Lines are 1-based. Files are relative to the fixture root.
