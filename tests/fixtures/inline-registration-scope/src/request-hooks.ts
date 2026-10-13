// Receives the instance and registers no route on it.
export function attachHooks(app: any, deps: any) {
  app.addHook('onRequest', async (req: any) => deps.log(req.id));
}

// A router this module also holds, whose instance carries the same name.
export async function hookStatusRoutes(app: any) {
  app.get('/hook-status', async () => ({ ok: true }));
}
