export function registerComputedRoutes(app: any) {
  app.get('/teams/:id/computed', async () => ({ computed: true }));
}
