import type { App, Handler, Request, Response } from "fixture-server";

function list(_request: Request, response: Response): void {
  response.send([]);
}

function more(...handlers: Handler[]): Handler[] {
  return handlers;
}

const BY_ID = "/by-id";
const BY_NAME = "/by-name";

export function withSpread(app: App): void {
  app.get(BY_ID, list, ...more(list));
  app.post(BY_NAME, ...more(list));
}
