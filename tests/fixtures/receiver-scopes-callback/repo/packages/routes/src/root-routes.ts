import type { App } from 'fixture-plugin-server';
export function registerRootRoutes(app: App) {
  app.get('/teams/:id/root', async () => ({ root: true }));
}
