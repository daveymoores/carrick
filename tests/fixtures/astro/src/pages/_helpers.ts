// `_`-prefixed files are excluded from Astro routing, so even though this
// exports something named like an HTTP method it must NOT become an endpoint.

export function GET() {
  return new Response("not a route");
}
