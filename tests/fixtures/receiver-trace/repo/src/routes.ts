import type { App, Request, Response } from "fixture-server";

function show(_request: Request, response: Response): void {
  response.send({});
}

export class Routes {
  constructor(private app: App) {}

  mount(): void {
    this.app.get("/c", show);
  }
}
