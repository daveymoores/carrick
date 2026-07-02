import { z } from "zod";

// Contract shapes shared across the platform monorepo. The zod schemas are the
// source of truth; the exported types are derived via z.infer.
export const PriceSchema = z.object({
  amount: z.number(),
  currency: z.string(),
});
export type Price = z.infer<typeof PriceSchema>;

export const VariantDetailSchema = z.object({
  id: z.string(),
  productId: z.string(),
  sku: z.string(),
  price: PriceSchema,
  inStock: z.boolean(),
});
export type VariantDetail = z.infer<typeof VariantDetailSchema>;

export interface Product {
  id: string;
  name: string;
  description: string;
  price: Price | null;
  tags: string[];
}

export interface PriceUpdated {
  productId: string;
  price: Price;
  effectiveAt: string;
}
