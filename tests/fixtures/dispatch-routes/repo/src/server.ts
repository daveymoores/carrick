import { createServer } from "node:http";
import type { IncomingMessage, ServerResponse } from "node:http";
import { api } from "./api";
import { isExportRoute } from "./routes";

function send(res: ServerResponse, body: string): void {
  res.end(body);
}

createServer((req, res) => {
  if (req.method === "GET" && req.url === "/orders") {
    send(res, "list");
  } else if (req.method === "POST" && req.url === "/orders") {
    send(res, "created");
  } else if (req.method === "GET" && req.url === "/status") {
    send(res, "up");
  }
});

function route(req: IncomingMessage, res: ServerResponse): void {
  const url = new URL(req.url ?? "/", "http://localhost");
  const pathname = url.pathname;
  if (pathname.startsWith("/api/")) {
    return api(req, res);
  }
  if (req.method === "POST" && isExportRoute(url)) {
    return send(res, "exported");
  }
  switch (pathname) {
    case "/profile":
      send(res, "profile");
      return;
  }
  if (req.method === "DELETE") {
    if (pathname === "/cache") {
      send(res, "cleared");
    }
  }
  switch (req.method) {
    case "GET":
    case "HEAD":
      if (pathname === "/feed") {
        send(res, "feed");
      }
  }
}

createServer(route);
