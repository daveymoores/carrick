import fastify from "fastify";

const app = fastify();

// HTTP PRODUCER. Fastify is the framework-agnostic proof point for this corpus
// (corpus-1 covered Express/NestJS; corpus-2 adds Fastify/Hono).

// Cross-repo contract type for edge 7.
// Producer here -> web-dashboard consumer expects this same Notification shape.
export interface Notification {
  id: string;
  message: string;
  read: boolean;
}

// GET /notifications/:id  — producer for edge 7 (consumer: web-dashboard).
// Cross-repo key normalises to http|GET|/notifications/:param.
app.get<Notification>("/notifications/:id", async (request, reply): Promise<Notification> => {
  const { id } = request.params;
  return { id, message: "your order shipped", read: false };
});

// GET /health  — ORPHAN PRODUCER (no consumer in corpus).
// Inline return-type literal `{ status: string }` — the anchor is the inline
// object type, not a named symbol (primary_type_symbol = null in ground truth).
app.get<{ status: string }>("/health", async (_request, _reply): Promise<{ status: string }> => {
  return { status: "ok" };
});

export default app;
