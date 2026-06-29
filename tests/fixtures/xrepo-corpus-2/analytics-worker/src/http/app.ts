// Hono HTTP PRODUCER — edge 8 (third framework, after Fastify + Express).
//
// `app.post('/track', c => c.json<TrackResult>(...))` registers POST /track.
// Request body is { path: string; userId: string }; response anchors on
// TrackResult. The matching consumer is web-dashboard's `fetch POST /track`.
// Cross-repo key: http|POST|/track. Capability tier (Hono producer is a real
// HTTP framework, framework-agnostic proof).

import { Hono } from "hono";

const app = new Hono();

// Request body shape for POST /track.
export interface TrackRequest {
  path: string;
  userId: string;
}

// Response contract for POST /track — the producer anchor type.
export interface TrackResult {
  ok: boolean;
}

// POST /track — record a tracked event, return { ok }.
app.post("/track", async (c) => {
  const body = await c.req.json<TrackRequest>();
  const result: TrackResult = { ok: Boolean(body.path && body.userId) };
  return c.json<TrackResult>(result);
});

export { app };
