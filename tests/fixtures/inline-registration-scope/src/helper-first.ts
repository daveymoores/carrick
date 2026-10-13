import { registerItemRoutes } from './team-items';

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      registerItemRoutes(api, deps);
    },
    { prefix: '/api/v1' },
  );
}
