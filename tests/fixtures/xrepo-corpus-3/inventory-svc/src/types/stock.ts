import { z } from "zod";

// The wire contract for stock adjustments is the zod schema; the type is
// derived from it.
export const StockAdjustSchema = z.object({
  sku: z.string(),
  delta: z.number(),
  reason: z.string(),
  orderId: z.string(),
});
export type StockAdjustCommand = z.infer<typeof StockAdjustSchema>;

export interface StockLevel {
  sku: string;
  warehouseId: string;
  onHand: number;
  reserved: number;
}

export interface PriceUpdatedEvent {
  productId: string;
  price: { amount: number; currency: string };
  effectiveAt: string;
}
