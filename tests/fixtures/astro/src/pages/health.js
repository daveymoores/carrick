// Astro endpoint in plain JavaScript: src/pages/health.js → /health

export function GET() {
  return new Response(JSON.stringify({ status: "ok" }), {
    headers: { "Content-Type": "application/json" },
  });
}
