import type { Handler } from "./types";

// Shape 3: the object is a module-scope const, and the export names it.
const worker: Handler = {
  async fetch(request: Request): Promise<Response> {
    return new Response(request.headers.get("host"));
  },
};

export default worker;
