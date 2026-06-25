// GraphQL resolver shapes for the orders gateway (producer side of #220).
//
// The schema lives in src/schema.graphql (schema-first, scanner-detectable).
// This module declares the TypeScript types behind those root fields so the
// sidecar resolves the request/response shapes for the type-enrichment work
// (#222): a generic wrapper (ApiResponse<T>), a nested object (Money), an
// optional field (note), and a union field (status).

// Nested object, reused inside Order.
export interface Money {
  amountCents: number;
  currency: string;
}

// Union field — discriminated on `kind`.
export type OrderStatus =
  | { kind: "placed"; placedAt: string }
  | { kind: "refunded"; refundedAt: string; reason?: string };

// The GraphQL `Order` response shape. Mirrors src/schema.graphql `type Order`.
//   - total: nested Money object
//   - status: union field
//   - note: optional field (present on producer; some consumers omit it)
export interface Order {
  id: string;
  total: Money;
  status: OrderStatus;
  note?: string;
}

// Generic response wrapper threaded through the resolvers (#222 generics case).
export interface ApiResponse<T> {
  data: T;
  errors: string[];
}

// query order(id): Order — producer for web-frontend `query order`.
export async function resolveOrder(id: string): Promise<ApiResponse<Order>> {
  return {
    data: {
      id,
      total: { amountCents: 4999, currency: "EUR" },
      status: { kind: "placed", placedAt: new Date().toISOString() },
      note: "gift wrap",
    },
    errors: [],
  };
}

// mutation refundOrder(id, reason): Order!
export async function resolveRefundOrder(
  id: string,
  reason?: string
): Promise<ApiResponse<Order>> {
  return {
    data: {
      id,
      total: { amountCents: 0, currency: "EUR" },
      status: { kind: "refunded", refundedAt: new Date().toISOString(), reason },
    },
    errors: [],
  };
}

// subscription orderUpdated: Order! — producer for web-frontend
// `subscription orderUpdated`. Returns the full Order (with the optional `note`).
export async function* resolveOrderUpdated(): AsyncGenerator<Order> {
  yield {
    id: "ord_1",
    total: { amountCents: 4999, currency: "EUR" },
    status: { kind: "placed", placedAt: new Date().toISOString() },
    note: "gift wrap",
  };
}
