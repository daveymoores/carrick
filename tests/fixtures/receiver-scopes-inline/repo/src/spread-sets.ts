import type { App } from 'fixture-plugin-server';
import { itemRoutes } from './items';

export async function api(app: App, flag: boolean) {
  app.register(
    async (api: App) => {
      await api.register(itemRoutes);
    },
    { ...(flag ? { prefix: '/x' } : {}) },
  );
}
