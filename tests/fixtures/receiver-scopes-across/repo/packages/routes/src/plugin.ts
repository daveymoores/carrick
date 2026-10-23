import type { App } from 'fixture-plugin-server';
import { orderRoutes } from './orders';

export interface RoutesOptions {
  basePrefix?: string;
  extra?: (scope: App) => Promise<void> | void;
}

// The routes of this package, registered under one scope. A host adds its
// own routes to the same scope through `extra`.
export async function routesPlugin(app: App, opts: RoutesOptions) {
  await app.register(
    async (api: App) => {
      await api.register(orderRoutes);
      if (opts.extra) await opts.extra(api);
    },
    { prefix: opts.basePrefix ?? '/api/v1' },
  );
}
