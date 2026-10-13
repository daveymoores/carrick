import { registerAllRoutes } from './all-routes';

export async function plugin(app: any, deps: any) {
  app.register( // first scope
    async (api: any) => {
      registerAllRoutes(api, deps); // first
    },
    { prefix: '/a' },
  );
  app.register( // second scope
    async (api: any) => {
      registerAllRoutes(api, deps); // second
    },
    { prefix: '/b' },
  );
}
