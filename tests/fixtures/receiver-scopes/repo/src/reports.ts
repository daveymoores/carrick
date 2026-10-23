import router, { App, Reply, Request } from "fixture-router";

function ok(_request: Request, reply: Reply): void {
  reply.send("ok");
}

export function reports(): App {
  const made = router();
  made.get("/daily", ok);
  return made;
}
