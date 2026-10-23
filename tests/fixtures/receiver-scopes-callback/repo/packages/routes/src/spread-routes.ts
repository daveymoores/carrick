import type { App } from 'fixture-plugin-server';
export function registerSpreadRoutes(app: App) {
  app.get('/teams/:id/spread', async () => ({ spread: true }));
}
