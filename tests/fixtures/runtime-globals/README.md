# runtime-globals

A Node service whose program loads no DOM library (`"lib": ["ES2022"]`,
`"types": ["node"]`). Its `fetch` is declared only by the runtime's types
package (`web-globals/fetch.d.ts`), so the store is asked for it under the
key the runtime's types give it, `node:web-globals/fetch`
(carrick#2445). `src/orders.ts` calls `fetch` through a helper; its caller
passes `"/api/orders"` and `"POST"`. The cassette answers that key.
