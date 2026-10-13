import { registerAllRoutes } from './all-routes';

export async function plugin(app: any) {
  app.register(
    async (api: any) => {
      // Registered as a plugin: the function is called with the instance.
      await api.register(registerAllRoutes);
    },
    { prefix: '/api/v1' },
  );
}
