import { t } from "./kit";

const late = "late";

// With no spread, a key written before a computed key may be set again by
// it: only the key written after it is read.
export const computedRouter = t.router({
  before: t.procedure.query(() => "before"),
  [late]: t.procedure.query(() => "late"),
  last: t.procedure.query(() => "last"),
});
