// Shared domain types for payments-svc

export interface Payment {
  id: string;
  orderId: number;
  amountCents: number;
  status: "pending" | "settled";
}

export interface CreatePayment {
  orderId: number;
  amountCents: number;
}

// POST /widgets — request-body direction fixture (widening). The endpoint accepts
// just { name }; the web-frontend caller sends a REQUIRED superset { name, note }.
// Request bodies flow caller → endpoint (consumer ⊑ producer), so a widening body
// is compatible. Response is byte-identical on both sides so the edge verdict is
// driven purely by the request pair.
export interface WidgetRequest {
  name: string;
}

export interface WidgetCreated {
  id: string;
}

// POST /invoices — request-body direction fixture (narrowing). The endpoint
// REQUIRES both invoiceId and amountCents; the caller omits the required
// amountCents. In the request direction (consumer ⊑ producer) this narrowing body
// is incompatible.
export interface InvoiceRequest {
  invoiceId: string;
  amountCents: number;
}

export interface InvoiceCreated {
  id: string;
}
