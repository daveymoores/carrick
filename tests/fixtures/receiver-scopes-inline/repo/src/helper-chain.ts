import type { App } from 'fixture-plugin-server';
import { registerAllRoutes } from './all-routes';

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      registerAllRoutes(api, deps);
    },
    { prefix: '/api/v1' },
  );
}
