import { makeClient } from "./lib/apiClient";
import { config } from "./config";

const catalogClient = makeClient(config.catalogUrl);

// price is optional here; the producer models "no price" as `price: null`.
// null is not undefined — the deliberate mismatch on this edge.
export interface ProductRecord {
  id: string;
  name: string;
  description: string;
  price?: { amount: number; currency: string };
  tags: string[];
}

export interface ProductPatch {
  name?: string;
  description?: string;
  tags?: string[];
}

export async function updateProduct(id: string, patch: ProductPatch): Promise<ProductRecord> {
  return catalogClient.patch<ProductRecord>(`/api/v2/products/${id}`, patch);
}
