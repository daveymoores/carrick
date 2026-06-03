// Next.js App Router: app/users/route.ts → /users
// Methods are named exports; GET and POST below become two endpoints.

interface User {
  id: string;
  name: string;
}

export async function GET(): Promise<Response> {
  const users: User[] = [{ id: "1", name: "Ada" }];
  return Response.json(users);
}

export async function POST(req: Request): Promise<Response> {
  const body = (await req.json()) as { name: string };
  const created: User = { id: "2", name: body.name };
  return Response.json(created, { status: 201 });
}

// Not an HTTP method — must be ignored by the deriver.
export const runtime = "edge";
