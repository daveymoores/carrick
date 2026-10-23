import type { App } from 'fixture-plugin-server';

// Routes written in the host's own service, registered on the scope the
// plugin of another package opens.
export function registerAuditRoutes(app: App) {
  app.get('/audit/events', async () => ({ events: [] as string[] }));
  app.get('/audit/events/:id', async () => ({ event: null }));
}
