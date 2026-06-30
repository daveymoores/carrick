// HTTP consumer for the dashboard. Both calls use ${process.env.*_URL} bases
// declared internal in carrick.json, so the cross-repo HTTP matcher fires.
//
//   GET  ${NOTIFICATIONS_URL}/notifications/:id -> Notification  [edge 7 CONSUMER]
//   POST ${ANALYTICS_URL}/track                 -> TrackResult   [edge 8 CONSUMER]

const NOTIFICATIONS_BASE = process.env.NOTIFICATIONS_URL ?? "";
const ANALYTICS_BASE = process.env.ANALYTICS_URL ?? "";

// Compatible with the notifications-svc Fastify producer Notification type.
export interface Notification {
  id: string;
  message: string;
  read: boolean;
}

// Request body for POST /track (compatible with the analytics-worker producer).
export interface TrackRequest {
  path: string;
  userId: string;
}

// Compatible with the analytics-worker Hono producer TrackResult type.
export interface TrackResult {
  ok: boolean;
}

// GET /notifications/:id — fetch one notification from notifications-svc.
export async function fetchNotification(id: string): Promise<Notification> {
  const res = await fetch(`${NOTIFICATIONS_BASE}/notifications/${id}`);
  if (!res.ok) {
    throw new Error(`Failed to fetch notification ${id}: ${res.status}`);
  }
  return res.json() as Promise<Notification>;
}

// POST /track — record a page view via analytics-worker.
export async function track(body: TrackRequest): Promise<TrackResult> {
  const res = await fetch(`${ANALYTICS_BASE}/track`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    throw new Error(`Failed to track: ${res.status}`);
  }
  return res.json() as Promise<TrackResult>;
}
