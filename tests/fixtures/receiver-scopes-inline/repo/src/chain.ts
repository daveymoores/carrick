import type { App } from 'fixture-plugin-server';
export function leaf(app: App) {
  app.get('/leaf', async () => []);
}

export function hop1(app: App) {
  leaf(app);
}

export function hop2(app: App) {
  hop1(app);
}

export function hop3(app: App) {
  hop2(app);
}

export function hop4(app: App) {
  hop3(app);
}
