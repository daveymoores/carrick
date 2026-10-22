import type { App } from 'fixture-plugin-server';
import { registerLeft, registerRight } from './pair';

export async function plugin(app: App, deps: any) {
  app.register(
    async (api: App) => {
      registerLeft(api);
      registerRight(api);
    },
    { prefix: '/api/v1' },
  );
}
