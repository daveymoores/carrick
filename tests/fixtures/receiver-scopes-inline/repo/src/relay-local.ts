import type { App } from 'fixture-plugin-server';
import { registerItemRoutes } from './team-items';

// Declared in the file that makes the scope: it registers nothing itself and
// hands the instance it was given on.
function relay(server: App, deps: any) {
  registerItemRoutes(server, deps);
}

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      relay(api, deps);
    },
    { prefix: '/api/v1' },
  );
}
