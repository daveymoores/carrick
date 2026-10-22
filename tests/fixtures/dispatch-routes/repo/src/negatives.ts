import { createServer } from "node:http";
import type { IncomingMessage, ServerResponse } from "node:http";
import { createServer as createLegacyServer } from "fixture-legacy-server";
import { listen } from "./listen";

const other = { method: "GET" };

createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  if (url.pathname !== "/admin") {
    res.end("no");
    return;
  }
  if (req.headers["x-prefix"] === "/edge") {
    res.end("edge");
  }
  if (req.url === "*") {
    res.end("star");
  }
  if (other.method === "GET" && req.url === "/mixed") {
    res.end("mixed");
  }
  if ((req.method === "GET" || req.method === "HEAD") && req.url === "/both") {
    res.end("both");
  }
  if ((req.method === "POST" || req.headers["x-admin"] === "yes") && req.url === "/either") {
    res.end("either");
  }
});

export function settings(): boolean {
  return window.location.pathname === "/settings";
}

listen((req: IncomingMessage, res: ServerResponse) => {
  if (req.url === "/wrapped") {
    res.end("wrapped");
  }
});

createLegacyServer((req, res) => {
  if (req.url === "/legacy") {
    res.end("legacy");
  }
});

createServer({ keepAlive: true }, (req, res) => {
  if (req.url === "/options") {
    res.end("options");
  }
});
