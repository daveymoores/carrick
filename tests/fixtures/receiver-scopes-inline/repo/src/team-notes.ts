import type { App } from 'fixture-plugin-server';
// The instance is the second parameter here.
export function registerNoteRoutes(deps: any, app: App) {
  app.get('/teams/:id/notes', async () => deps.notes.list());
}
