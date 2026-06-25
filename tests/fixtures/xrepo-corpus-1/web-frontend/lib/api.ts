// API client for the web frontend.
// Types here deliberately differ from the orders-monorepo producer in ways that
// create a cross-repo type incompatibility on the orders endpoint:
//   - OrderView.id is string (producer Order.id is number)
//   - OrderView has no amountCents field (producer has amountCents: number)
//
// The payments call is type-compatible with the payments-svc producer.

const ORDERS_BASE = process.env.NEXT_PUBLIC_ORDERS_URL ?? "";
const PAYMENTS_BASE = process.env.NEXT_PUBLIC_PAYMENTS_URL ?? "";

// Intentionally INCOMPATIBLE with orders-monorepo's Order type:
//   producer: Order { id: number; amountCents: number; currency: string }
//   consumer: OrderView { id: string; currency: string }  ← id is string, amountCents absent
export interface OrderView {
  id: string;
  currency: string;
}

// Compatible with the payments-svc producer response shape.
export interface Payment {
  id: string;
  orderId: number;
  amountCents: number;
  status: string;
}

export interface CreatePaymentRequest {
  orderId: number;
  amountCents: number;
}

// GET /orders/:id — fetches a single order by id.
// NOTE: id is typed as string here; the producer emits id as number (incompatible edge).
export async function fetchOrder(id: string): Promise<OrderView> {
  const res = await fetch(`${ORDERS_BASE}/orders/${id}`);
  if (!res.ok) {
    throw new Error(`Failed to fetch order ${id}: ${res.status}`);
  }
  return res.json() as Promise<OrderView>;
}

// POST /payments — creates a new payment.
// Request and response types are compatible with the payments-svc producer.
export async function createPayment(
  body: CreatePaymentRequest
): Promise<Payment> {
  const res = await fetch(`${PAYMENTS_BASE}/payments`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    throw new Error(`Failed to create payment: ${res.status}`);
  }
  return res.json() as Promise<Payment>;
}
