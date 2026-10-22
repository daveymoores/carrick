# dispatch-routes

The routes a hand-written dispatch states (carrick#2474 R4a). Every name is
made up. `repo/` is one root service; `library-claims.json` is the store
answer replayed for it (`node:http`'s `createServer` labelled `receive` with
its `request` fields, and `fixture-legacy-server`'s without them).

`dispatch.expected.json` is the answer key: each route on the line its
literal is compared on, with its method and how it reaches the request, the
`startsWith` scope, and every comparison that states no route, by reason.
It was written from the source, file by file:

- `worker.ts`: a platform handler nothing calls. Entry routes; methods from
  `&&` operands; none in the `switch` and the table; table routes at their
  key lines.
- `server.ts`, `api.ts`, `routes.ts`: handlers handed to `createServer`.
  Label routes; `DELETE /cache` and `GET`/`HEAD /feed` from an enclosing
  method guard; `POST /orders/export` from the caller's test of the
  predicate in `routes.ts`; the `/api/` scope delegating to `api`.
- `middleware.ts`: the minimal fixture of carrick#2139. Two entry routes on
  their own lines.
- `negatives.ts`: a negated path, a header compared, a name literal, a
  method on another value, methods under `||` (with each other, and with
  another test), a browser global, a handler
  handed to an in-repo wrapper, a label with no `request`, and a handler in
  another slot than the label's.

`records.expected.json` is the key of the handler records (carrick#2474
R4b): one record per request handler, a shared record for a comparison
several handlers reach, each comparison forced or asked. Every route of
`dispatch.expected.json` is a comparison of a record. The files added for
it (R4a's are unchanged):

- `session.ts` + `pages.ts`: a `/logout` check in an exported helper that
  three exported page loaders call. One shared record, asked once.
- `edge.ts`: an entry handler whose path comes from an in-repo function
  (`normalise(request.url)`): a hole in the handler, asked, `GET` forced.
- `nested.ts`: a path whose branch compares two methods
  (`methods_in_branch`): one question among both methods.
- `mounted.ts`: `stats` handed to `fixture-router`'s `route`, whose label
  has a `name` slot: the path is forced and no method is stated, so nothing
  is asked. `use` takes its handler in another slot than the call's:
  `handler_slot_mismatch`. The closure handed to `route("/files", …)`
  compares `trimmed(req.path)`: a hole in a label handler, asked.
- `fake.ts`: an exported factory nothing calls, whose closure is handed to
  the in-repo `listen` and compares `parse(req.url)`: the hole attaches
  through nesting to the factory.
- `report.ts` + `check.ts`: a predicate called from an entry handler with
  an unread argument (`resolve(request)`): the chain stops in the handler,
  so the predicate's comparison is the handler's.
