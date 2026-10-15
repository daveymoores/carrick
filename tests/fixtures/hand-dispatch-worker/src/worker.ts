const ITEM_PREFIX = "/items/";

type Item = { id: string; name: string };

const items = new Map<string, Item>();

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

async function audit(action: string, id: string): Promise<void> {
  const body = JSON.stringify({ action, id });
  await fetch("/audit", { method: "POST", body });
}

export default {
  async fetch(request: Request): Promise<Response> {
    const { pathname } = new URL(request.url);

    if (pathname === "/items" && request.method === "GET") {
      return json([...items.values()]);
    }

    if (pathname === "/items" && request.method === "POST") {
      const item = (await request.json()) as Item;
      items.set(item.id, item);
      await audit("create", item.id);
      return json(item, 201);
    }

    if (pathname.startsWith(ITEM_PREFIX) && request.method === "DELETE") {
      const id = pathname.slice(ITEM_PREFIX.length);
      items.delete(id);
      await audit("delete", id);
      return json({ deleted: id });
    }

    switch (pathname) {
      case "/health":
        return json({ ok: true });
      default:
        return json({ error: "not found" }, 404);
    }
  },
};
