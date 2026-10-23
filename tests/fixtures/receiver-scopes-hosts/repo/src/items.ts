import type { App } from "fixture-twin-server";

export async function items(app: App) {
  app.get("/items", (_request, reply) => reply.send([]));
}
