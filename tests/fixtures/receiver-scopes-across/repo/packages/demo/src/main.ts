import server from 'fixture-plugin-server';
import { routesPlugin } from '@fx/routes';

const app = server();

// A third host, its options spread from a value the source does not state.
export async function boot(deps: { read: Record<string, unknown> }) {
  await app.register(routesPlugin, { ...deps.read });
  return app;
}
