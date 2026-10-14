export function registerBuiltRoutes(app: any) {
  app.get('/teams/:id/built', async () => ({ built: true }));
}
