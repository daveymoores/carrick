import type { App } from 'fixture-plugin-server';

// Routes a host registers on the scope it is handed.
export function registerAdminRoutes(app: App) {
  app.get('/admin/users', async () => ({ users: [] as string[] }));
  app.post('/admin/users', async () => ({ created: true }));
}
