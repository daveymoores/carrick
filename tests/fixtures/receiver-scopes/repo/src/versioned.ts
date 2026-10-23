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

// A default stands only for a caller that omits the value.
function mountOmitted(app: App, prefix: string = "/omit-default"): void {
  app.register((scoped) => {
    scoped.get("/pong", ok);
  }, { prefix });
}

function mountStated(app: App, prefix: string = "/stated-default"): void {
  app.register((scoped) => {
    scoped.get("/pang", ok);
  }, { prefix });
}

export function mountDefaults(app: App): void {
  mountOmitted(app, "/given");
  mountOmitted(app);
  mountStated(app, "/only");
}
