export function registerSpreadRoutes(app: any) {
  app.get('/teams/:id/spread', async () => ({ spread: true }));
}
