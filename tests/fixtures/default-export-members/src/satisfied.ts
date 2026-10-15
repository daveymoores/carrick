import type { Handler } from "./types";

// Shape 2: the same object, checked against a type where it is exported.
export default {
  async fetch(request: Request): Promise<Response> {
    return new Response(request.method);
  },
} satisfies Handler;
