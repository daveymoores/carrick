// Shape 1: the default export is an object literal with no name of its own.
export default {
  async fetch(request: Request): Promise<Response> {
    return new Response(request.url);
  },
};
