// The helper is declared in the file that makes the scope.
function registerLocalRoutes(server: any) {
  server.get('/local', async () => []);
}

export async function plugin(app: any, deps: any) {
  app.register(
    async (api: any) => {
      registerLocalRoutes(api);
    },
    { prefix: '/api/v1' },
  );
}
