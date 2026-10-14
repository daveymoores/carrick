import { registerBothRoutes } from './both-routes';

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      registerBothRoutes(api, deps);
    },
    { prefix: '/api/v1' },
  );
}
