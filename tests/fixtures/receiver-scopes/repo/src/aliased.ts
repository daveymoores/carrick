import router, { Reply, Request } from "fixture-router";

function ok(_request: Request, reply: Reply): void {
  reply.send("ok");
}

const web = router();
web.get("/w", ok);

// Exported under another name than the local it was built in.
export const aliased = web;
