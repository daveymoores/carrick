import { registerItemRoutes } from './team-items';

declare function other(): any;

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
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
