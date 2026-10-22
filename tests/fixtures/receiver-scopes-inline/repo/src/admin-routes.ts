import type { App } from 'fixture-plugin-server';
export function registerAdminRoutes(app: App) {
  app.get('/admin/users', async () => []);
}
