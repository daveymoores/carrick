// Next.js App Router: app/health/route.ts → /health

export async function GET(): Promise<Response> {
  return Response.json({ status: "ok" });
}
