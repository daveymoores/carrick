import type { App } from 'fixture-plugin-server';
import { registerAllRoutes } from './all-routes';

export async function plugin(app: App) {
  app.register(
    async (api: App) => {
      // Registered as a plugin: the function is called with the instance.
      await api.register(registerAllRoutes);
    },
    { prefix: '/api/v1' },
  );
}
