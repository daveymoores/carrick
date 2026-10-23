import type { Admin } from "fixture-twin-server";

export async function list(admin: Admin) {
  admin.get("/list", (_request, reply) => reply.send([]));
}
