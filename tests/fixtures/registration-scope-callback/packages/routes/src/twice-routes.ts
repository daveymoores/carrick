export function registerTwiceRoutes(app: any) {
  app.get('/teams/:id/twice', async () => ({ twice: true }));
}
