async function readNotes() {
  return { notes: [] as string[] };
}

export function registerNoteRoutes(app: any) {
  app.get('/teams/:id/notes', readNotes);
}
