import type { App } from 'fixture-plugin-server';
async function readNotes() {
  return { notes: [] as string[] };
}

export function registerNoteRoutes(app: App) {
  app.get('/teams/:id/notes', readNotes);
}
