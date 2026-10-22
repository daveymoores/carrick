import type { App } from 'fixture-plugin-server';
import {
  routesPlugin,
  registerTagRoutes,
  registerRootRoutes,
  twicePlugin,
  computedPlugin,
  builtPlugin,
  spreadPlugin,
  registerTwiceRoutes,
  registerComputedRoutes,
  registerBuiltRoutes,
  registerSpreadRoutes,
  registerLegacyRoutes,
} from '@fx/routes';
import { registerExtraRoutes } from './extra';

const extraKey = 'registerExtra';

function builtOptions() {
  return { registerExtra: (scope: App) => registerBuiltRoutes(scope) };
}

export async function start(app: App, deps: any) {
  await app.register(routesPlugin, {
    prefix: '/api/v1',
    registerExtra: async (scope: App) => {
      registerExtraRoutes(scope, deps);
      registerTagRoutes(scope);
      registerLegacyRoutes(scope);
    },
    registerOnRoot: (root: App) => registerRootRoutes(root),
  });

  // Options that never write the key: a spread in them may hold more, and
  // cannot replace what the call above writes.
  await app.register(routesPlugin, { ...deps.routeOptions });

  // Supplied twice: which call serves the scope is not one answer.
  await app.register(twicePlugin, { registerExtra: (scope: App) => registerTwiceRoutes(scope) });
  await app.register(twicePlugin, { registerExtra: (scope: App) => registerTwiceRoutes(scope) });

  // A computed key after the written one may replace it.
  await app.register(computedPlugin, {
    registerExtra: (scope: App) => registerComputedRoutes(scope),
    [extraKey]: (scope: App) => scope,
  });

  // One call writes the option, another builds its options elsewhere.
  await app.register(builtPlugin, { registerExtra: (scope: App) => registerBuiltRoutes(scope) });
  await app.register(builtPlugin, builtOptions());

  // A spread after the written key may replace it.
  await app.register(spreadPlugin, {
    registerExtra: (scope: App) => registerSpreadRoutes(scope),
    ...deps.spreadOptions,
  });
}
