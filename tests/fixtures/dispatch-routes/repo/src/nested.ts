export function items(request: Request): Response {
  const url = new URL(request.url);
  if (url.pathname === "/items") {
    if (request.method === "GET") return new Response("list");
    if (request.method === "POST") return new Response("made", { status: 201 });
  }
  return new Response("no", { status: 404 });
}
