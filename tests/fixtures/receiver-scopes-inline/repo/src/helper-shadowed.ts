import type { App } from 'fixture-plugin-server';
import { registerItemRoutes } from './team-items';

declare function other(): any;

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      api.log.info('scope');
      {
        // Another binding that carries the parameter's name.
        const api = other();
        registerItemRoutes(api, deps);
      }
    },
    { prefix: '/api/v1' },
  );
}
