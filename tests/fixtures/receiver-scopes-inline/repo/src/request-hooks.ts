import type { App } from 'fixture-plugin-server';
// Receives the instance and registers no route on it.
export function attachHooks(app: App, deps: any) {
  app.addHook('onRequest', async (req: any) => deps.log(req.id));
}

// A router this module also holds, whose instance carries the same name.
export async function hookStatusRoutes(app: App) {
  app.get('/hook-status', async () => ({ ok: true }));
}
