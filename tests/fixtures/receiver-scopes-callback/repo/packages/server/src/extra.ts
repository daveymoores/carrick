import type { App } from 'fixture-plugin-server';
import { registerNoteRoutes, registerAdminRoutes } from '@fx/routes';

export function registerExtraRoutes(app: App, deps: any) {
  registerNoteRoutes(app);
  registerAdminRoutes(app);
  app.get('/teams/:id/audit', async () => deps.audit());
}
