import router, { Reply, Request } from "fixture-router";

function list(_request: Request, reply: Reply): void {
  reply.send([]);
}

export const api = router();
api.get("/items", list);
api.post("/items", list);
