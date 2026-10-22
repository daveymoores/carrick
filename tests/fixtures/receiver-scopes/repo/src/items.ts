import { App, Reply, Request } from "fixture-router";

function ok(_request: Request, reply: Reply): void {
  reply.send("ok");
}

function deep(app: App): void {
  app.get("/leaf", ok);
}

export function items(app: App): void {
  app.get("/items", ok);
  // A plugin registered inside a plugin: both prefixes.
  app.register(deep, { prefix: "/deep" });
}
