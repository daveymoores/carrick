import make, { Handler, Request, Response, router } from "fixture-server";
import { withSpread } from "./spread";

function ok(_request: Request, response: Response): void {
  response.send("ok");
}

function handlers(...list: Handler[]): Handler[] {
  return list;
}

// The paths are constants so that no model row sits at these lines.
const PLAIN = "/plain";
const SPREAD = "/spread";
const WITH_PATH = "/const";

// A local instance: no spread, a spread of a call after a handler, and a
// spread at the handler slot after the path.
const app = make();
app.get(PLAIN, ok, ok);
app.get(SPREAD, ok, ...handlers(ok));
app.post(WITH_PATH, ...handlers(ok));

// A parameter.
withSpread(app);

// A router whose members no declaration verifies for.
const api = router();
api.get(PLAIN, {}, ok);
api.get(SPREAD, {}, ...handlers(ok));
api.post(WITH_PATH, {}, ...handlers(ok));
