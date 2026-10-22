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
  method on another value, methods under `||`, a browser global, a handler
  handed to an in-repo wrapper, a label with no `request`, and a handler in
  another slot than the label's.
