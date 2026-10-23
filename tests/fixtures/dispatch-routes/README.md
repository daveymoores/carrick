# dispatch-routes

The routes a hand-written dispatch chooses (carrick#2474 R4a, R4b). Every
name is made up. `repo/` is one root service; `library-claims.json` is the
store answer replayed for it (`node:http`'s `createServer` labelled
`receive` with its `request` fields, `fixture-legacy-server`'s without
them, and `fixture-router`'s `route` with a `name` (path) slot).

`records.expected.json` is the key of the handler records: one record per
request handler, a shared record for a comparison several handlers reach,
each comparison settled by the code (path and method forced, or a forced
path with no one method qualifying: `no_method`) or asked, and where each
comparison's route stands (`places`: its line, whether the compared path is
the path served, and the functions a reader row folds within).

`handler-answers.json` is one repeat of the live test's stored answers
(carrick#2474 comment 6358002149), keyed by the scan's site ids. It still
holds the answers to `negatives.ts` and `server.ts:20`, whose methods the
model picked wrongly there: under the ruling that the method is the code's
fact, those comparisons are settled `no_method` and never asked, so those
answers are never read.

`rows.expected.json` is the key of the scan's dispatch rows (each
`file:line METHOD path source`) and possible calls (`file:line reason`)
with those answers: every route R4a stated is a row, an asked comparison
or a possible call here. No row carries a method the code did not choose.

The R4a files, written from the source file by file:

- `worker.ts`: a platform handler nothing calls. Entry comparisons; methods
  from `&&` operands (`GET /home`, `POST /orders`: rows); none in the
  `switch` and the table (`no_method`); table keys at their key lines.
- `server.ts`, `api.ts`, `routes.ts`: handlers handed to `createServer`.
  Label comparisons, path forced: facts where one method qualifies
  (`DELETE /cache` from an enclosing guard, `POST /orders/export` from the
  caller's test of the predicate in `routes.ts`); `/feed` under `case
  "GET": case "HEAD":` and `/profile` qualify no one method (`no_method`);
  the `/api/` scope delegating to `api`.
- `middleware.ts`: the minimal fixture of carrick#2139. Two entry routes on
  their own lines.
- `negatives.ts`: a negated path, a header compared, a name literal, a
  method on another value, methods under `||` (with each other, and with
  another test: `no_method`), a browser global, a handler handed to an
  in-repo wrapper, a label with no `request`, and a handler in another slot
  than the label's.

The files added for the handler records (R4b):

- `session.ts` + `pages.ts`: a `/logout` check in an exported helper that
  three exported page loaders call. One shared record, asked once.
- `edge.ts`: an entry handler whose path comes from an in-repo function
  (`normalise(request.url)`): a hole in the handler, asked, `GET` forced.
- `nested.ts`: a path whose branch compares two methods
  (`methods_in_branch`): asked whether it is a route, never which method;
  a route is `no_method`.
- `mounted.ts`: `stats` handed to `fixture-router`'s `route`, whose label
  has a `name` slot: the path is forced and no method is stated, so nothing
  is asked; the handler is registered under a path, so `owner_unknown`.
  `use` takes its handler in another slot than the call's:
  `handler_slot_mismatch`. The closure handed to `route("/files", …)`
  compares `trimmed(req.path)`: a hole in a label handler, asked, and
  `owner_unknown` for the same path slot.
- `fake.ts`: an exported factory nothing calls, whose closure is handed to
  the in-repo `listen` and compares `parse(req.url)`: the hole attaches
  through nesting to the factory, and is `owner_unknown` (the closure is
  handed to a call no label names).
- `report.ts` + `check.ts`: a predicate called from an entry handler with
  an unread argument (`resolve(request)`): the chain stops in the handler,
  so the predicate's comparison is the handler's.

The file added for carrick#2693:

- `action.ts`: a method literal compared with something other than the
  request's method member (a body's `action`, a header) beside a path
  comparison: on the entry arm with a member read off the request
  (`req.body.action` in `patch`) and off another value (`body.action` in
  `act`), on a hole in an entry handler (`clean(request.url)`) and on a
  hole in a label handler (`clean(req.url)`). Each is asked and answered `route` in band
  (`handler-answers.json`, written for this file, not a live repeat), and
  each ends `no_method`: the literal chooses no method, and the record
  lists no method to the model (nor `other.method` in `negatives.ts`).
