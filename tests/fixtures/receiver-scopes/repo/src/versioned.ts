import { App, Reply, Request } from "fixture-router";

function ok(_request: Request, reply: Reply): void {
  reply.send("ok");
}

// A prefix the helper takes as a parameter: each call below fills it.
function mountVersioned(app: App, prefix: string): void {
  app.register((scoped) => {
    scoped.get("/ping", ok);
  }, { prefix });
}

export function mountVersions(app: App): void {
  mountVersioned(app, "/v9");
  mountVersioned(app, "/v10");
}
