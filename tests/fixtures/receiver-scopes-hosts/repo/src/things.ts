import type { Hub } from "fixture-split-server";

export async function things(hub: Hub) {
  hub.get("/things", (_request, reply) => reply.send([]));
}
