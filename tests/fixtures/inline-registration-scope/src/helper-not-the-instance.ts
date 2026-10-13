import { registerAdminRoutes } from './admin-routes';
import { registerItemRoutes } from './team-items';
import { registerNoteRoutes } from './team-notes';

declare function wrap(instance: any): any;

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      // A member of the instance, a value made from it, and a spread.
      registerItemRoutes(api.inner, deps);
      registerNoteRoutes(deps, wrap(api));
      registerAdminRoutes(...[api]);
    },
    { prefix: '/api/v1' },
  );
}
