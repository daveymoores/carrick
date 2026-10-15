import { createServer } from "node:http";
import type { IncomingMessage, ServerResponse } from "node:http";

const ROUTES: Record<string, string> = {
  "/about": "About us",
  "/contact": "Write to us",
};

function send(res: ServerResponse, status: number, body: string): void {
  res.statusCode = status;
  res.end(body);
}

function route(req: IncomingMessage, res: ServerResponse): void {
  const pathname = new URL(req.url ?? "/", "http://localhost").pathname;
  switch (pathname) {
    case "/health":
      send(res, 200, "ok");
      return;
    case "/ready":
    case "/live":
      send(res, 200, "yes");
      return;
  }
  const page = ROUTES[pathname];
  if (page) {
    send(res, 200, page);
    return;
  }
  send(res, 404, "not found");
}

createServer(route).listen(8080);

createServer((req, res) => {
  if (req.url === "/version") {
    send(res, 200, "1.0.0");
    return;
  }
  send(res, 404, "not found");
}).listen(8081);
