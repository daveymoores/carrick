# default-export-members

carrick#2396: the function members of an object a module exports are indexed
whatever form the export takes, and a member call through the import the
module gives that object reaches the member's definition.

One shape per file:

- `src/anonymous.ts`: `export default { async fetch(request) { … } }`. The
  object has no name of its own, so its members are keyed `default.<member>`,
  and a default import names it `default` when it looks a member up.
- `src/satisfied.ts`: the same object, written `export default { … } satisfies
  Handler`. TypeScript wrappers and parentheses are read through.
- `src/bound.ts`: `const worker: Handler = { … }; export default worker`. The
  members are keyed by the name the module declares, `worker.<member>`.
- `src/listed.ts`: `export { jobs }`, written before `const jobs = ({ … }) as
  Jobs`. The export may come before the declaration.
- `src/renamed.ts`: `export { queue as tasks }`. Still keyed `queue.<member>`,
  the declared name, which is what an importer of `tasks` resolves to.
- `src/declared.ts`: `export const scheduled = { … } satisfies Jobs`.
- `src/assigned.js`: `const store = { … }; module.exports = store`.

`src/caller.ts` imports each through the import its module gives it and calls
the member. `src/bounds.ts` holds the bounds: a module-local object, an
exported `let` (a later assignment can replace it), and a function-local
`const` that shares an exported name. None of their members is indexed.

Two files define `default.fetch`, so the test matches definitions by
`(file_path, name)`: the merge re-keys the second (carrick#582).

The LLM is replayed from `__llm__/` and states nothing.
