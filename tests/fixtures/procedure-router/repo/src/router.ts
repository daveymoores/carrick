import { t } from "./kit";
import { healthProcedure } from "./health";
import { ordersRouter } from "./orders";
import { looseRouter } from "./loose";
import { accountsRouter as accountRoutes } from "./accounts";
import users from "./users";
import { computedRouter } from "./computed";
import { legacyRouter, pickedRouter } from "./legacy";

export const appRouter = t.router({
  ping: t.procedure.query(() => "pong"),
  health: healthProcedure(),
  orders: ordersRouter,
  loose: looseRouter,
  // A member the label states no method for: no HTTP operation.
  events: t.procedure.stream(() => "tick"),
  accounts: accountRoutes,
  users,
  computed: computedRouter,
  legacy: legacyRouter,
  picked: pickedRouter,
});
