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
} from '@fx/routes';
import { registerExtraRoutes } from './extra';

const extraKey = 'registerExtra';

function builtOptions() {
  return { registerExtra: (scope: any) => registerBuiltRoutes(scope) };
}

export async function start(app: any, deps: any) {
  await app.register(routesPlugin, {
    prefix: '/api/v1',
    registerExtra: async (scope: any) => {
      registerExtraRoutes(scope, deps);
      registerTagRoutes(scope);
    },
    registerOnRoot: (root: any) => registerRootRoutes(root),
  });

  // Supplied twice: which call serves the scope is not one answer.
  await app.register(twicePlugin, { registerExtra: (scope: any) => registerTwiceRoutes(scope) });
  await app.register(twicePlugin, { registerExtra: (scope: any) => registerTwiceRoutes(scope) });

  // A computed key after the written one may replace it.
  await app.register(computedPlugin, {
    registerExtra: (scope: any) => registerComputedRoutes(scope),
    [extraKey]: (scope: any) => scope,
  });

  // One call writes the option, another builds its options elsewhere.
  await app.register(builtPlugin, { registerExtra: (scope: any) => registerBuiltRoutes(scope) });
  await app.register(builtPlugin, builtOptions());

  // A spread after the written key may replace it.
  await app.register(spreadPlugin, {
    registerExtra: (scope: any) => registerSpreadRoutes(scope),
    ...deps.spreadOptions,
  });
}
