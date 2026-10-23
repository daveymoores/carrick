import type { App } from 'fixture-plugin-server';
export function registerComputedRoutes(app: App) {
  app.get('/teams/:id/computed', async () => ({ computed: true }));
}
