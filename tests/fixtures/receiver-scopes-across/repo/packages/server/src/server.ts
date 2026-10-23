import server from 'fixture-plugin-server';
import type { App } from 'fixture-plugin-server';
import { routesPlugin, registerAdminRoutes } from '@fx/routes';
import { registerAuditRoutes } from './audit';

declare const base: string;
declare const flag: boolean;

const app = server();

// The main host: its options state the plugin's own prefix option (which
// only the plugin reads), spread a conditional, and add routes to the
// plugin's scope through `extra`.
export async function start() {
  await app.register(routesPlugin, {
    basePrefix: flag ? `${base}api/v1` : '/api/v1',
    ...(flag ? { other: 1 } : {}),
    extra: async (scope: App) => {
      registerAdminRoutes(scope);
      registerAuditRoutes(scope);
    },
  });
  return app;
}
