// The instance is the second parameter here.
export function registerNoteRoutes(deps: any, app: any) {
  app.get('/teams/:id/notes', async () => deps.notes.list());
}
