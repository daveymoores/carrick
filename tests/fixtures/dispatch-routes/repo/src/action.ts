import { createServer } from "node:http";

function clean(url: string): string {
  return new URL(url, "http://localhost").pathname.toLowerCase();
}

export async function act(request: Request): Promise<Response> {
  const body = await request.json();
  const url = new URL(request.url);
  if (body.action === "PUT" && url.pathname === "/drafts") {
    return new Response("saved");
  }
  if (body.action === "DELETE" && clean(request.url) === "/drafts/all") {
    return new Response("cleared");
  }
  return new Response("not found", { status: 404 });
}

export function patch(req: { url: string; body: { action: string } }): string {
  if (req.body.action === "PATCH" && req.url === "/drafts/one") {
    return "patched";
  }
  return "not found";
}

createServer((req, res) => {
  if (req.headers["x-action"] === "DELETE" && clean(req.url ?? "/") === "/purge") {
    res.end("purged");
  }
});
