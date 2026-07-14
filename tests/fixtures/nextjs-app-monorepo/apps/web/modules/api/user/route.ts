// Handler implemented OUTSIDE the app/ tree; the app route file re-exports it.
// This file itself must derive NO route (it is not under an app root).
export async function POST(): Promise<Response> {
  return new Response(JSON.stringify({ created: true }));
}

export async function OPTIONS(): Promise<Response> {
  return new Response(null);
}
