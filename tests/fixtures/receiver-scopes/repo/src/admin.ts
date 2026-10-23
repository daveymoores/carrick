import router, { App, Reply, Request } from "fixture-router";

function ok(_request: Request, reply: Reply): void {
  reply.send("ok");
}

const admin = router();
admin.get("/stats", ok);

export function mountAdmin(app: App): void {
  app.route("/admin", admin);
}
