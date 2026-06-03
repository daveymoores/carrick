// Next.js App Router dynamic segment: app/users/[id]/route.ts → /users/:id

interface User {
  id: string;
  name: string;
}

export async function GET(
  _req: Request,
  { params }: { params: { id: string } },
): Promise<Response> {
  const user: User = { id: params.id, name: "Ada" };
  return Response.json(user);
}

export async function DELETE(
  _req: Request,
  { params }: { params: { id: string } },
): Promise<Response> {
  return Response.json({ deleted: params.id });
}
