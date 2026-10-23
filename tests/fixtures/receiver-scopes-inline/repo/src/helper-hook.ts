import type { App } from 'fixture-plugin-server';
import { attachHooks } from './request-hooks';

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      attachHooks(api, deps);
    },
    { prefix: '/api/v1' },
  );
}
