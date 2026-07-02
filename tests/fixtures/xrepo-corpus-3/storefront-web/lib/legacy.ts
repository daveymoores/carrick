const CATALOG_BASE = process.env.NEXT_PUBLIC_CATALOG_URL ?? "http://localhost:4001";

// Never migrated off the retired v1 API. The catalog serves only /api/v2 —
// this call has no producer and must not match the v2 routes.
export interface LegacyProduct {
  id: string;
  name: string;
  priceCents: number;
}

export async function fetchLegacyProduct(id: string): Promise<LegacyProduct> {
  const res = await fetch(`${CATALOG_BASE}/api/v1/products/${id}`);
  return res.json() as Promise<LegacyProduct>;
}
