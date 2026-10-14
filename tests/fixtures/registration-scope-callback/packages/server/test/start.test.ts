// A test supplies the same option: a scan reads no test file, so this is not
// a second supplier.
import { routesPlugin, registerTagRoutes } from '@fx/routes';

export async function setup(app: any) {
  await app.register(routesPlugin, { registerExtra: (scope: any) => registerTagRoutes(scope) });
}
