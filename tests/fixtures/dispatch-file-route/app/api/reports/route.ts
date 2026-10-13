export async function GET(request: Request) {
  const view = new URL(request.url).searchParams.get('view');
  if (view === 'totals') {
    return Response.json({ totals: [] });
  }
  if (view === 'trend') {
    return Response.json({ trend: [] });
  }
  if (view === 'export') {
    return Response.json({ rows: [] });
  }
  return Response.json({ error: 'unknown view' }, { status: 400 });
}
