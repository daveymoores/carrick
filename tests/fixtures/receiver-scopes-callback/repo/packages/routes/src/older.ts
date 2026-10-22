import type { App } from 'fixture-plugin-server';
import { legacyRoutes } from './legacy';

// The package already mounts this file's routes under a prefix of its own.
export async function legacyPlugin(app: App) {
  await app.register(legacyRoutes, { prefix: '/legacy' });
}
