import type { App } from 'fixture-plugin-server';
// The helper is declared in the file that makes the scope.
function registerLocalRoutes(server: App) {
  server.get('/local', async () => []);
}

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      registerLocalRoutes(api);
    },
    { prefix: '/api/v1' },
  );
}
