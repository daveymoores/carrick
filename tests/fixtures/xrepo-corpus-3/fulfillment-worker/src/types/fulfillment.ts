// What the worker believes arrives on the dispatch queue. dispatchAfter is
// typed Date, but the publisher sends an ISO string — the deliberate mismatch.
export interface DispatchJob {
  orderId: string;
  warehouseId: string;
  dispatchAfter: Date;
}

export interface OrderStatusChanged {
  orderId: string;
  status: "picked" | "dispatched" | "delivered";
  occurredAt: string;
}

// Narrower view of the catalog's VariantDetail (productId deliberately
// omitted — safe producer-extra-fields narrowing).
export interface VariantView {
  id: string;
  sku: string;
  price: { amount: number; currency: string };
  inStock: boolean;
}
