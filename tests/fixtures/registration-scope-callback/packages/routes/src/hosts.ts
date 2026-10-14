// Plugins whose host callback the scan cannot follow: each keeps the paths
// of the routes it reaches.
export async function twicePlugin(app: any, opts: any) {
  await app.register(async (api: any) => {
    await opts.registerExtra(api);
  }, { prefix: '/twice' });
}

export async function computedPlugin(app: any, opts: any) {
  await app.register(async (api: any) => {
    await opts.registerExtra(api);
  }, { prefix: '/computed' });
}

export async function builtPlugin(app: any, opts: any) {
  await app.register(async (api: any) => {
    await opts.registerExtra(api);
  }, { prefix: '/built' });
}

export async function spreadPlugin(app: any, opts: any) {
  await app.register(async (api: any) => {
    await opts.registerExtra(api);
  }, { prefix: '/spread' });
}
