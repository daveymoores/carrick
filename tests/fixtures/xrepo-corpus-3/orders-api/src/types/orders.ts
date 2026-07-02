export interface CreateOrder {
  customerId: string;
  items: { sku: string; quantity: number }[];
}

export interface OrderCreated {
  orderId: string;
  status: "pending" | "confirmed";
  total: { amount: number; currency: string };
}

export interface TimelineEvent {
  at: string;
  status: string;
  note?: string;
}

// Sent to inventory when an order reserves stock.
export interface StockAdjustCommand {
  sku: string;
  delta: number;
  reason: string;
  orderId: string;
}

// Enqueued for the fulfillment worker. dispatchAfter is an ISO timestamp
// string on the wire — the worker's expectation of a Date is the bug.
export interface DispatchRequest {
  orderId: string;
  warehouseId: string;
  dispatchAfter: string;
}

export interface OrderStatusChanged {
  orderId: string;
  status: "picked" | "dispatched" | "delivered";
  occurredAt: string;
}

export interface OrderDigest {
  date: string;
  orderCount: number;
  totalAmount: number;
}
