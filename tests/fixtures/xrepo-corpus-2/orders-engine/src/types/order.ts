// Owned domain types for orders-engine — the spec-of-record payload shapes.
// These EXACT field names are the cross-repo answer key; do not rename.

// Nested money object — shared shape across the order.placed contract.
export interface Money {
  amountCents: number;
  currency: string;
}

// Kafka `order.placed` publisher payload (edge 1, compatible with the
// notifications-svc subscriber's OrderPlacedEvent). `note?` is optional.
export interface OrderPlaced {
  id: string;
  total: Money;
  note?: string;
}

// Generic transport wrapper around the Kafka payload. The publisher sends an
// Envelope<OrderPlaced>; the INNER OrderPlaced is the contract type (unwrap
// stress at the call site — the expected resolved_type is OrderPlaced, not this).
export interface Envelope<T> {
  v: number;
  data: T;
}

// GraphQL `query order` producer result (edge 5, compatible with web-dashboard).
export interface OrderView {
  id: string;
  total: Money;
}

// GraphQL `subscription orderEvents` producer payload (edge 6). Discriminated
// union; producer only emits "placed" | "shipped" (the web-dashboard consumer
// widens this to include "cancelled" → INCOMPATIBLE, missing-union-member).
export interface OrderEvent {
  orderId: string;
  kind: "placed" | "shipped";
}

// GraphQL `mutation cancelOrder` producer result (ORPHAN — no consumer doc
// references it in the corpus).
export interface CancelResult {
  id: string;
  cancelled: boolean;
}
