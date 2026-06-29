// Publisher payload types for billing-svc's pub/sub call sites.

// Kafka `order.placed` publisher payload — DELIBERATELY INCOMPATIBLE with the
// notifications-svc subscriber contract: `total` here is a bare cents `number`,
// whereas OrderPlacedEvent.total is `{ amountCents: number; currency: string }`.
// This is edge 2 (payload-shape mismatch).
export interface OrderPlaced {
  id: string;
  total: number;
}

// NATS `payment.captured` publisher payload — ORPHAN (no subscriber in corpus).
export interface PaymentCaptured {
  orderId: string;
  amountCents: number;
}
