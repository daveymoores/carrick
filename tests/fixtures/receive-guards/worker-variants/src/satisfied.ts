type Handler = { fetch(request: Request): Promise<Response> };

export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/status") {
      return Response.json({ ok: true });
    }
    return new Response("not found", { status: 404 });
  },
} satisfies Handler;
