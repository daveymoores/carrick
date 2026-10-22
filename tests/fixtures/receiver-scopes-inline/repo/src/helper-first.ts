import type { App } from 'fixture-plugin-server';
import { registerItemRoutes } from './team-items';

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      registerItemRoutes(api, deps);
    },
    { prefix: '/api/v1' },
  );
}
