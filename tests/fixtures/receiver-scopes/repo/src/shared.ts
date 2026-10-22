import router, { Reply, Request } from "fixture-router";

function ok(_request: Request, reply: Reply): void {
  reply.send("ok");
}

export const shared = router();
shared.get("/ping", ok);
