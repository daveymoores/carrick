import { Items } from "./items";

// The composition root: no `send`, so the client sends with `fetch`.
export const items = new Items({ base: "/v2" });
