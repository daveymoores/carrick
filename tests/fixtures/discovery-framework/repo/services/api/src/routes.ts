import type { Handler, ServerInstance } from "fixture-server";

function wrap(app: ServerInstance, run: Handler): Handler {
  return async (request, reply) => run(request, reply);
}

export async function itemRoutes(app: ServerInstance) {
  app.get("/items", async (_request, reply) => reply.send([]));
  app.post("/items", wrap(app, async (request, reply) => reply.send(request.body)));
}
