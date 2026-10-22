import type { App } from 'fixture-plugin-server';
import { hop4 } from './chain';

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      hop4(api);
    },
    { prefix: '/api/v1' },
  );
}
