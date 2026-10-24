import router, { Reply, Request } from "fixture-router";

function list(_request: Request, reply: Reply): void {
  reply.send([]);
}

export const jobs = router();
jobs.get("/jobs", list);
