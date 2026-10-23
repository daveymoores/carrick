import router, { App, Reply, Request } from "fixture-router";
import { users } from "./users";
import { items } from "./items";
import { shared } from "./shared";
import { mountAdmin } from "./admin";
import { reports } from "./reports";
import { withOptions } from "./options";

function ok(_request: Request, reply: Reply): void {
  reply.send("ok");
}

function bare(app: App): void {
  app.get("/bare", ok);
}

const app = router();
// The root: no prefix.
app.get("/health", ok);
// A router built in another module, mounted under a prefix.
app.route("/v1/users", users);
// A plugin the member runs with a scoped app, under the prefix its options state.
app.register(items, { prefix: "/api" });
// A plugin written in place.
app.register((scoped) => {
  scoped.get("/inline", ok);
}, { prefix: "/in" });
// One router mounted twice: served under both.
app.route("/a", shared);
app.route("/b", shared);
// A helper that mounts on the app it is handed.
mountAdmin(app);
// A router an own factory builds and returns.
app.route("/r", reports());
// A scope the member returns, held in a local and chained.
const v2 = app.basePath("/v2");
v2.get("/x", ok);
app.basePath("/v3").get("/y", ok);
// A plugin run with no options: no prefix.
app.register(bare);
// An option the code calls with the app (waits on carrick#2432).
withOptions(app);
// A router handed to a member no scope claim names: served somewhere the
// trace cannot say.
const sub = router();
sub.get("/s", ok);
app.attach(sub);
