import got from "got";
import { VariantView } from "./types/fulfillment";

const CATALOG_BASE = process.env.CATALOG_URL ?? "http://localhost:4001";

export async function fetchVariant(productId: string, variantId: string): Promise<VariantView> {
  return got.get(`${CATALOG_BASE}/api/v2/products/${productId}/variants/${variantId}`).json<VariantView>();
}
