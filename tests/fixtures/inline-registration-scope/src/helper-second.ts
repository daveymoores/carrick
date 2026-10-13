import { registerNoteRoutes } from './team-notes';

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      await registerNoteRoutes(deps, api);
    },
    { prefix: '/api/v1' },
  );
}
