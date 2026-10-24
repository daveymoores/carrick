// An arrow-const export checked against the route module's handler type with
// `satisfies`: the wrapper states a type, the arrow is still the handler, and
// the module serves the one verb it narrows to.

type ActionArgs = { request: Request };
type ActionFunction = (args: ActionArgs) => Promise<{ ok: boolean }>;

export const action = (async ({ request }: ActionArgs) => {
  if (request.method !== "PUT") {
    throw new Response("Method Not Allowed", { status: 405 });
  }
  return { ok: true };
}) satisfies ActionFunction;
