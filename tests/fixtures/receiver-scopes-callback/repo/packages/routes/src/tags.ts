import type { App } from 'fixture-plugin-server';
export function registerTagRoutes(app: App) {
  app.get('/teams/:id/tags', async () => ({ tags: [] as string[] }));
}
