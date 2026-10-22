import type { App } from 'fixture-plugin-server';
import { hop3 } from './chain';

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      hop3(api);
    },
    { prefix: '/api/v1' },
  );
}
