import { registerAdminRoutes } from './admin-routes';
import { registerItemRoutes } from './team-items';

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      registerItemRoutes(api, deps);
      // The instance the plugin itself was given, not the scope's.
      registerAdminRoutes(app);
    },
    { prefix: '/api/v1' },
  );
}
