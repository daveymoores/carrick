export function leaf(app: any) {
  app.get('/leaf', async () => []);
}

export function hop1(app: any) {
  leaf(app);
}

export function hop2(app: any) {
  hop1(app);
}

export function hop3(app: any) {
  hop2(app);
}

export function hop4(app: any) {
  hop3(app);
}
