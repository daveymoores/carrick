import { t } from "./kit";

// A nested router exported as the module's default.
const usersRouter = t.router({
  me: t.procedure.query(() => ({})),
});

export default usersRouter;
