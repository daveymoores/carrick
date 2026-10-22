import type { App } from 'fixture-plugin-server';
export function registerItemRoutes(app: App, deps: any) {
  app.get('/teams/:id/items', async () => deps.items.list());
}
