type Handler = { fetch(request: Request): Promise<Response> };

const worker: Handler = {
  async fetch(request: Request): Promise<Response> {
    const path = new URL(request.url).pathname.toLowerCase();
    if (path === "/items") {
      return Response.json([]);
    }
    return new Response("not found", { status: 404 });
  },
};

export default worker;
