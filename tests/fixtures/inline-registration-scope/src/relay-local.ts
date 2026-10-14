import { registerItemRoutes } from './team-items';

// Declared in the file that makes the scope: it registers nothing itself and
// hands the instance it was given on.
function relay(server: any, deps: any) {
  registerItemRoutes(server, deps);
}

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      relay(api, deps);
    },
    { prefix: '/api/v1' },
  );
}
