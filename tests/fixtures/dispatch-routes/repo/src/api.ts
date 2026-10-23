import type { IncomingMessage, ServerResponse } from "node:http";

export function api(req: IncomingMessage, res: ServerResponse): void {
  const { pathname } = new URL(req.url ?? "/", "http://localhost");
  if (req.method === "GET" && pathname === "/api/items") {
    res.end("items");
  } else if (req.method === "PUT" && pathname === "/api/items") {
    res.end("saved");
  }
}
