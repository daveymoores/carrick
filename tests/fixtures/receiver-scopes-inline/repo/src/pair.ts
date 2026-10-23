import type { App } from 'fixture-plugin-server';
// Two helpers in one module, each handed the instance.
export function registerLeft(app: App) {
  app.get('/left', async () => []);
}

export function registerRight(app: App) {
  app.get('/right', async () => []);
}
