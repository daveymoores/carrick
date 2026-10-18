# `class-client-construction`

Fixture for carrick#2452: a class client whose base is a constructor
parameter. When only this file can construct the class, and every
construction passes the same value, the parameter reads as that value.

## The shape

`src/clients.ts` declares three classes, none exported, each constructed at
module scope and the instance exported. `src/use.ts` calls a method on each.

| Class | Construction | `src/use.ts` row |
|---|---|---|
| `Api`: a parameter property `base` | `new Api("/api/v1")` | `GET /api/v1/users` |
| `Items`: `this.base = base` in the constructor | `new Items(process.env.ITEMS_URL!)` | base is the environment variable `ITEMS_URL` (undeclared), path `${process.env.ITEMS_URL}/items` |
| `Pinger`: a parameter property `root` | `new Pinger("/east")` and `new Pinger("/west")` | the constructions disagree: the row keeps the field's own name, not `/east/ping` or `/west/ping` |

`ITEMS_URL` is undeclared on purpose: an environment read the code makes is
an expected find.

The cassettes hold no rows, so every row is stated by the source.
