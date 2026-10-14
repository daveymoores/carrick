export async function orderRoutes(app: any) {
  app.get('/orders', async () => ({ orders: [] as string[] }));
}
