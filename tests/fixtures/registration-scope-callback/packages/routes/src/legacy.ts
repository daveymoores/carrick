export default function registerLegacyRoutes(app: any) {
  app.get('/teams/:id/legacy', async () => ({ legacy: true }));
}
