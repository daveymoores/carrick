import server, { App } from "fixture-twin-server";
import { items } from "./items";

async function local(app: App) {
  app.get("/local", (_request, reply) => reply.send([]));
}

const app = server();
// Chained calls start where the call they are made on does.
app.register(local, { prefix: "/one" }).register(local, { prefix: "/two" });
app.register(items, { prefix: "/three" }).register(items, { prefix: "/four" });
