import { registerNoteRoutes } from './team-notes';

// Another file declares a local function of the same name under its own scope.
function relay(server: any, deps: any) {
  registerNoteRoutes(deps, server);
}

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      relay(api, deps);
    },
    { prefix: '/b' },
  );
}
