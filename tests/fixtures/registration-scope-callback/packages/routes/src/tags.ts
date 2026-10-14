export function registerTagRoutes(app: any) {
  app.get('/teams/:id/tags', async () => ({ tags: [] as string[] }));
}
