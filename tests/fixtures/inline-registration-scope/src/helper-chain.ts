import { registerAllRoutes } from './all-routes';

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      registerAllRoutes(api, deps);
    },
    { prefix: '/api/v1' },
  );
}
