// Two helpers in one module, each handed the instance.
export function registerLeft(app: any) {
  app.get('/left', async () => []);
}

export function registerRight(app: any) {
  app.get('/right', async () => []);
}
