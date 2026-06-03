// Astro dynamic endpoint: src/pages/posts/[id].ts → /posts/:id

import type { APIRoute } from "astro";

interface Post {
  id: string;
  title: string;
}

export const GET: APIRoute = ({ params }) => {
  const post: Post = { id: params.id ?? "0", title: "Hello" };
  return new Response(JSON.stringify(post), {
    headers: { "Content-Type": "application/json" },
  });
};
