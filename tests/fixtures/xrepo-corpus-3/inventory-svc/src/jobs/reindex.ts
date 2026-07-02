// Nightly cache warm: hits this service's own HTTP surface over localhost.
// An intra-repo self-call — not a cross-repo consumer, must not be extracted
// as a data call.
export async function warmStockCache(warehouseIds: string[], skus: string[]): Promise<void> {
  for (const wid of warehouseIds) {
    for (const sku of skus) {
      await fetch(`http://localhost:4002/warehouses/${wid}/stock/${sku}`);
    }
  }
}
