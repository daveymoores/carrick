import type { App } from 'fixture-plugin-server';
export function registerTwiceRoutes(app: App) {
  app.get('/teams/:id/twice', async () => ({ twice: true }));
}
