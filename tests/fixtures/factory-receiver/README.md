# factory-receiver

A receiver bound from a factory call (carrick#1146).

`packages/app` is the only scanned service. Its callers bind a `const` from a
call to a function (`const client = createClient()`) and call a member on it.
Nothing at the binding states a class. The declaration of the function called
does, in one of two ways:

- a return type annotation that names a class, or `Promise` of one where the
  call is awaited;
- with no annotation, a body whose only statement returns `new C(…)` or a call
  to another such function, at most three functions deep.

`@fixture/sdk` publishes two classes that declare the same member names, so
the package-surface join (carrick#781) cannot say which class a receiver is.
Only the factory's own declaration can.

Four callers in `packages/app/src/commands.ts` record the member edge
(`showRun`, `showRemoteRun`, `listRuns`, `listShared`). The others are the
answer key for what states no class: a promise never awaited, an interface, a
union, a body with two `return` statements, a `let`, a destructured binding, a
factory that is a parameter, and a chain four functions deep.

Driven by `tests/factory_receiver_test.rs`.
