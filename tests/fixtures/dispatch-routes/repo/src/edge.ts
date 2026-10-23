function normalise(url: string): string {
  return new URL(url).pathname.toLowerCase();
}

export async function handle(request: Request): Promise<Response> {
  const path = normalise(request.url);
  if (request.method === "GET" && path === "/reports") return new Response("reports");
  return new Response("not found", { status: 404 });
}
