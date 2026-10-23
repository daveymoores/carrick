import server from 'fixture-plugin-server';
import { routesPlugin } from '@fx/routes';

const app = server();

// A second host, its options a closed literal.
export async function boot() {
  await app.register(routesPlugin, { basePrefix: '/api/v1' });
  return app;
}
