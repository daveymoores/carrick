// Wrapped-const export shape (the handler is built by a HOF, not declared as
// `export async function GET`): the method must still come from the export name.
const withApiWrapper = (opts: { handler: () => Promise<Response> }) => opts.handler;

export const GET = withApiWrapper({
  handler: async () => new Response(JSON.stringify({ ok: true })),
});
