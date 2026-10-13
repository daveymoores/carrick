export function registerItemRoutes(app: any, deps: any) {
  app.get('/teams/:id/items', async () => deps.items.list());
}
