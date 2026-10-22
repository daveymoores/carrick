import type { App, Request, Response } from "fixture-server";

function list(_request: Request, response: Response): void {
  response.send([]);
}

export function users(app: App): void {
  app.get("/users", list);
}
