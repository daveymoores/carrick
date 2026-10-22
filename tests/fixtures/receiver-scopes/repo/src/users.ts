import router, { Reply, Request } from "fixture-router";

function list(_request: Request, reply: Reply): void {
  reply.send([]);
}

export const users = router();
users.get("/", list);
users.post("/:id", list);
