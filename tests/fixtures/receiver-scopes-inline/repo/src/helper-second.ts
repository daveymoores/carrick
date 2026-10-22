import type { App } from 'fixture-plugin-server';
import { registerNoteRoutes } from './team-notes';

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      await registerNoteRoutes(deps, api);
    },
    { prefix: '/api/v1' },
  );
}
