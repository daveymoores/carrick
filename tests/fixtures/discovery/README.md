# Discovery fixture (carrick#2431)

`repo/` is one service (`services/web`) whose calls reach labelled library
exports through in-repo functions, for every protocol. `library-claims.json`
is the store's answer for its fixture packages, in the `/library-claims`
answer format (`lib.dom` stands for the compiler's default library until the
store answers it).

`sites.expected.json` is the answer key: the record discovery writes per
site, sorted by file and span.

| Field | Meaning |
|---|---|
| `file`, `line` | The call, repo-relative, 1-based line |
| `span_start`, `span_end` | The call expression's SWC span in the file parsed on its own (byte offset + 1) |
| `callee` | The callee as written |
| `end` | The labelled call the walk reached: the store `package`, `export`, `member` (`null` for the export itself), `role`, `op` (the claim's op for that member, `null` for a construction), and that call's `file`, `line`, `span_start`, `span_end`. `null` when the walk stopped |
| `chain` | Each in-repo function the walk entered, outermost first: the definition key, its file and line. `<callback>` is a function written at the call and run by the callee |
| `stop` | Why a walk that entered code ended with no labelled export: a `HoleReason`, `interface_member`, `injected_callee`, `cycle` or `no_body` (a definition whose body the pass did not read). `null` when `end` is set |

A site that reaches several labelled calls has one record per labelled call.
Two sites can share one end (`traced-load.ts:4`: the `traced(…)` call and the
`fetch` it runs). A reader that folds records by their end must dedupe them.
