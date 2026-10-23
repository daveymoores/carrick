import type { App } from 'fixture-plugin-server';
import { registerAllRoutes } from './all-routes';

export async function plugin(app: App, deps: any) {
  app.register( // first scope
    async (api: App) => {
      registerAllRoutes(api, deps); // first
    },
    { prefix: '/a' },
  );
  app.register( // second scope
    async (api: App) => {
      registerAllRoutes(api, deps); // second
    },
    { prefix: '/b' },
  );
}
