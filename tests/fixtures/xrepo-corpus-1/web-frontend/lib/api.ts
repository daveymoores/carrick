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

// POST /widgets request body — a REQUIRED superset of what the endpoint accepts
// ({ name, note } vs the producer's { name }). In the request direction
// (consumer ⊑ producer) this widening body is compatible.
export interface CreateWidgetBody {
  name: string;
  note: string;
}

export interface WidgetCreated {
  id: string;
}

// POST /invoices request body — OMITS the endpoint's required amountCents
// ({ invoiceId } vs the producer's { invoiceId, amountCents }). In the request
// direction this narrowing body is incompatible.
export interface CreateInvoiceBody {
  invoiceId: string;
}

export interface InvoiceCreated {
  id: string;
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

// POST /widgets — creates a widget via payments-svc. The request body is a
// required superset of what the endpoint accepts, so the request pair is
// compatible; the response is byte-identical, so the edge reads compatible.
export async function createWidget(
  body: CreateWidgetBody
): Promise<WidgetCreated> {
  const res = await fetch(`${PAYMENTS_BASE}/widgets`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    throw new Error(`Failed to create widget: ${res.status}`);
  }
  return res.json() as Promise<WidgetCreated>;
}

// POST /invoices — creates an invoice via payments-svc. The request body omits
// the endpoint's required amountCents, so the request pair is incompatible and
// the edge reads incompatible even though the response is byte-identical.
export async function createInvoice(
  body: CreateInvoiceBody
): Promise<InvoiceCreated> {
  const res = await fetch(`${PAYMENTS_BASE}/invoices`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    throw new Error(`Failed to create invoice: ${res.status}`);
  }
  return res.json() as Promise<InvoiceCreated>;
}
