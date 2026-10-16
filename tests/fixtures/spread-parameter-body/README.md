# `spread-parameter-body`

Fixture for carrick#2411: a request body that writes a key and then spreads
the function's own parameter (`{ op, ...args }`). The spread may overwrite
the key, so the request line states no value for it. At a call that passes an
object literal, the literal's own keys say what the spread puts in place, so
the call states the value its body sends.

## The shape

- `src/send-op.ts`: `sendOp(op, args)` sends `{ op, ...args }`.
- `src/gateway.ts`: the `Gateway` class.
  - `run(op, input)` sends `{ op, ...input }`.
  - `purgeAll(scope)` hands its own parameter to `run`'s spread.
  - `refreshBoth()` calls `run` twice with two literals.
  - `rpc(message)` sends the object it is handed, and `search(query)` hands it
    `{ op: "search", ...query }`.
  - `lock(opts)` sends `{ ...opts, op: "lock" }`: the key comes after the
    spread.
- `src/refused.ts`: wrappers whose spread the reading does not prove: two
  parameter spreads, a spread of a parameter's key, a parameter assigned again
  and a parameter written through.
- `src/purge.ts`, `src/alpha.ts`, `src/beta.ts`, `src/unread.ts`, `src/ops.ts`
  and `src/jobs.ts` are the callers.

Every URL is read through a module constant, so each request line states its
own row. Each wrapper has its own parameter names, because writes through a
name are read across the whole file. The cassette gives every caller a model
row whose `op` is invented from the member name, so a correct value can only
come from the source.

## The answer key

| site | `op` |
|---|---|
| `purge.ts:4` `sendOp("purge", {})` | `purge` |
| `purge.ts:5` `sendOp("purge", { id })` | `purge` |
| `purge.ts:6` `sendOp("purge", { op: "evict" })` | `evict`, the runtime overwrite |
| `purge.ts:7` `sendOp("purge", flag ? { id: "1" } : {})` | `purge` |
| `alpha.ts:4`, `beta.ts:4` | `alpha`, `beta` |
| `send-op.ts:6`, the request line | none |
| `unread.ts:9`-`12`: a local, a call's result, a spread argument, no argument | none |
| `ops.ts:4` `gateway.purgeAll({})` | `purge-all` |
| `ops.ts:5` `gateway.refreshBoth()` | two rows, `refresh-a` and `refresh-b` |
| `ops.ts:6` `gateway.search({ q })` | `search` |
| `ops.ts:7` `gateway.lock({ op: "unlock" })` | `lock` |
| `ops.ts:12` `gateway.purgeAll(given)`, `given` a parameter nothing fills | none |
| `jobs.ts:4`-`7` | none |
