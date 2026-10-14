export function registerRootRoutes(app: any) {
  app.get('/teams/:id/root', async () => ({ root: true }));
}
