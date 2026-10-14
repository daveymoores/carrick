export function registerLegacyRoutes(app: any) {
  app.get('/teams/:id/legacy', async () => ({ legacy: true }));
}

export const legacyRoutes = registerLegacyRoutes;
