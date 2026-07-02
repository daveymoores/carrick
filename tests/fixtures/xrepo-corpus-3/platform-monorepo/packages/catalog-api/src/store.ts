import { Product, VariantDetail } from "@meridian/contracts";

// In-memory stand-ins for the catalog database. Static fixture: bodies are
// illustrative, the types are the contract.
export async function loadProduct(id: string): Promise<Product> {
  return {
    id,
    name: "Meridian mug",
    description: "Stoneware, 350ml",
    price: { amount: 1450, currency: "EUR" },
    tags: ["kitchen", "ceramics"],
  };
}

export async function loadVariant(productId: string, variantId: string): Promise<VariantDetail> {
  return {
    id: variantId,
    productId,
    sku: `SKU-${variantId}`,
    price: { amount: 1450, currency: "EUR" },
    inStock: true,
  };
}

export async function applyProductPatch(id: string, patch: unknown): Promise<Product> {
  const current = await loadProduct(id);
  return { ...current, ...(patch as Partial<Product>) };
}

export async function removeProduct(id: string): Promise<void> {
  void id;
}
