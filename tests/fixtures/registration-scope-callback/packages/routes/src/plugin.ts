import { orderRoutes } from './orders';

export interface RoutesOptions {
  prefix?: string;
  registerExtra?: (scope: any) => Promise<void> | void;
  registerOnRoot?: (app: any) => void;
}

// The routes of this package, registered under one prefix. A host adds its
// own routes to the same scope through `registerExtra`.
export async function routesPlugin(app: any, opts: RoutesOptions) {
  await app.register(
    async (api: any) => {
      await api.register(orderRoutes);
      if (opts.registerExtra) await opts.registerExtra(api);
      opts.registerOnRoot?.(app);
    },
    { prefix: opts.prefix ?? '/api/v1' },
  );
}
