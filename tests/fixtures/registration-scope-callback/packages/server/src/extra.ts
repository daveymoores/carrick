import { registerNoteRoutes, registerAdminRoutes } from '@fx/routes';

export function registerExtraRoutes(app: any, deps: any) {
  registerNoteRoutes(app);
  registerAdminRoutes(app);
  app.get('/teams/:id/audit', async () => deps.audit());
}
