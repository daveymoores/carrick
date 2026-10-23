import type { App } from 'fixture-plugin-server';
// Two route functions in one file: the host registers the first on the
// scope, and the second on its root app.
export function registerAdminRoutes(app: App) {
  app.get('/admin/keys', async () => ({ keys: [] as string[] }));
}

export function registerPublicRoutes(app: App) {
  app.get('/public/keys', async () => ({ keys: [] as string[] }));
}
