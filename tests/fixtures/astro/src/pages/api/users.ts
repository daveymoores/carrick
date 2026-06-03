// Astro endpoint: src/pages/api/users.ts → /api/users
// Astro has no forced /api prefix — "api" here is just a directory segment.
// Methods come from named exports (mix of `function` and `const` forms).

import type { APIRoute } from "astro";

interface User {
  id: string;
  name: string;
}

export const GET: APIRoute = () => {
  const users: User[] = [{ id: "1", name: "Ada" }];
  return new Response(JSON.stringify(users), {
    headers: { "Content-Type": "application/json" },
  });
};

export async function POST({ request }: { request: Request }): Promise<Response> {
  const body = (await request.json()) as { name: string };
  const created: User = { id: "2", name: body.name };
  return new Response(JSON.stringify(created), { status: 201 });
}

// Not an HTTP method — must be ignored by the deriver.
export const prerender = false;
