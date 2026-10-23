import type { App } from 'fixture-plugin-server';
import { registerItemRoutes } from './team-items';

// Another file declares a local function of the same name under its own scope.
function relay(server: App, deps: any) {
  registerItemRoutes(server, deps);
}

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      relay(api, deps);
    },
    { prefix: '/a' },
  );
}
