# open-member-any

Synthetic two-service monorepo for carrick#2591: `api` answers
`POST /checkout` with a `Checkout { id: string; meta: any }`, a type whose
one member the author declares `any`. `web` casts the fetched body to a
stated type twice: `IdAsNumber` (line 13) reads `id` as a number, a closed
member and a real mismatch, and `MetaX` (line 18) reads `meta.x`, the open
member, which is never a mismatch. `http-server` is a hand-written
declaration, as in `retype-http-client`.

Used by `tests/open_member_any_test.rs`.
