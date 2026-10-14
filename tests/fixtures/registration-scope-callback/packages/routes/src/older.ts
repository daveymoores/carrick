import { legacyRoutes } from './legacy';

// The package already mounts this file's routes under a prefix of its own.
export async function legacyPlugin(app: any) {
  await app.register(legacyRoutes, { prefix: '/legacy' });
}
