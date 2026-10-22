import type { App } from 'fixture-plugin-server';
import { registerItemRoutes } from './team-items';

export async function plugin(app: App, deps: any) {
  app.register( // first scope
    async (api: App) => {
      registerItemRoutes(api, deps); // first
    },
    { prefix: '/a' },
  );
  app.register( // second scope
    async (api: App) => {
      registerItemRoutes(api, deps); // second
    },
    { prefix: '/b' },
  );
}
