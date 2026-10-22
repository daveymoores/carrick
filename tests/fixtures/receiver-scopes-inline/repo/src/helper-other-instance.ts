import type { App } from 'fixture-plugin-server';
import { registerAdminRoutes } from './admin-routes';
import { registerItemRoutes } from './team-items';

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      registerItemRoutes(api, deps);
      // The instance the plugin itself was given, not the scope's.
      registerAdminRoutes(app);
    },
    { prefix: '/api/v1' },
  );
}
