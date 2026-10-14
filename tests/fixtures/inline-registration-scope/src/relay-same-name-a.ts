import { registerItemRoutes } from './team-items';

// Another file declares a local function of the same name under its own scope.
function relay(server: any, deps: any) {
  registerItemRoutes(server, deps);
}

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      relay(api, deps);
    },
    { prefix: '/a' },
  );
}
