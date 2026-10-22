const ROUTES = {
  "/assets/logo": "logo",
  "/assets/font": "font",
};

function reply(text: string): Response {
  return new Response(text);
}

export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === "/home") {
      return reply("home");
    } else if (request.method === "POST" && url.pathname === "/orders") {
      return reply("created");
    }
    switch (url.pathname) {
      case "/health":
      case "/ready":
        return reply("ok");
      case "/version":
        return reply("1");
    }
    const asset = ROUTES[url.pathname as keyof typeof ROUTES];
    if (asset) {
      return reply(asset);
    }
    return reply("not found");
  },
};
