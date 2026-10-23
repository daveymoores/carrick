import type { App } from 'fixture-plugin-server';
import { registerBothRoutes } from './both-routes';

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      registerBothRoutes(api, deps);
    },
    { prefix: '/api/v1' },
  );
}
