import { registerItemRoutes } from './team-items';

export async function plugin(app: any, deps: any) {
  app.register( // first scope
    async (api: any) => {
      registerItemRoutes(api, deps); // first
    },
    { prefix: '/a' },
  );
  app.register( // second scope
    async (api: any) => {
      registerItemRoutes(api, deps); // second
    },
    { prefix: '/b' },
  );
}
