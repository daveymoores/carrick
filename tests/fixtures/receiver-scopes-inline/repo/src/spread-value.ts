import type { App } from 'fixture-plugin-server';
import { itemRoutes } from './items';

export async function api(app: App, deps: { options: { prefix?: string } }) {
  app.register(
    async (api: App) => {
      await api.register(itemRoutes);
    },
    { ...deps.options },
  );
}
