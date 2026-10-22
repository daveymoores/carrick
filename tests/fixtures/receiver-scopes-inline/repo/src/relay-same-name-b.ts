import type { App } from 'fixture-plugin-server';
import { registerNoteRoutes } from './team-notes';

// Another file declares a local function of the same name under its own scope.
function relay(server: App, deps: any) {
  registerNoteRoutes(deps, server);
}

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      relay(api, deps);
    },
    { prefix: '/b' },
  );
}
