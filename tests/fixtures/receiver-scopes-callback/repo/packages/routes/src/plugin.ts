import type { App } from 'fixture-plugin-server';
import { orderRoutes } from './orders';

export interface RoutesOptions {
  prefix?: string;
  registerExtra?: (scope: App) => Promise<void> | void;
  registerOnRoot?: (app: App) => void;
}

// The routes of this package, registered under one prefix. A host adds its
// own routes to the same scope through `registerExtra`.
export async function routesPlugin(app: App, opts: RoutesOptions) {
  await app.register(
    async (api: App) => {
      await api.register(orderRoutes);
      if (opts.registerExtra) await opts.registerExtra(api);
      opts.registerOnRoot?.(app);
    },
    { prefix: opts.prefix ?? '/api/v1' },
  );
}
