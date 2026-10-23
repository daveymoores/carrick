import type { App } from 'fixture-plugin-server';
export function registerLegacyRoutes(app: App) {
  app.get('/teams/:id/legacy', async () => ({ legacy: true }));
}

export const legacyRoutes = registerLegacyRoutes;
