import type { App } from 'fixture-plugin-server';
// Plugins whose host callback the scan cannot follow: each keeps the paths
// of the routes it reaches.
export async function twicePlugin(app: App, opts: any) {
  await app.register(async (api: App) => {
    await opts.registerExtra(api);
  }, { prefix: '/twice' });
}

export async function computedPlugin(app: App, opts: any) {
  await app.register(async (api: App) => {
    await opts.registerExtra(api);
  }, { prefix: '/computed' });
}

export async function builtPlugin(app: App, opts: any) {
  await app.register(async (api: App) => {
    await opts.registerExtra(api);
  }, { prefix: '/built' });
}

export async function spreadPlugin(app: App, opts: any) {
  await app.register(async (api: App) => {
    await opts.registerExtra(api);
  }, { prefix: '/spread' });
}
