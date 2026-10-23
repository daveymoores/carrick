import type { App } from 'fixture-plugin-server';
export function registerBuiltRoutes(app: App) {
  app.get('/teams/:id/built', async () => ({ built: true }));
}
