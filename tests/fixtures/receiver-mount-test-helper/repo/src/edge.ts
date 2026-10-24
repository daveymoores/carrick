import router, { Reply, Request } from "fixture-router";

function ping(_request: Request, reply: Reply): void {
  reply.send([]);
}

export const edge = router();
edge.get("/ping", ping);
