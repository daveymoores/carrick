import { t } from "./kit";

// A nested router another module imports under a name of its own.
export const accountsRouter = t.router({
  show: t.procedure.query(() => ({})),
});
