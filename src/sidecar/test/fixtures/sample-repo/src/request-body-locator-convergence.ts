/**
 * Fixture for the request_body TEXT-LOCATOR convergence regression.
 *
 * Mirrors two cross-repo corpus shapes where a Gemini `expression_text` +
 * `expression_line` locator (not a byte span) lands on a PROPERTY NAME, a
 * whole PropertyAssignment, or a declared-through-`JSON.stringify` IDENTIFIER
 * rather than the payload expression itself, and used to resolve the useless
 * `string` result instead of the payload shape:
 *
 *  (a) `fetch(url, { ..., body: <serialized param> })` — a locator text
 *      anchored on the property (a bare `"body"`, or the whole `key: value`
 *      property source) at the property's line resolves the property NAME /
 *      PropertyAssignment,
 *      which types as the assigned value (`string`), not the payload. The
 *      param/property/argument names deliberately COLLIDE (`body`) — that
 *      collision is what made `matchByText`'s exact match land on the wrong
 *      node in practice.
 *  (b) `const body = JSON.stringify(payload); sendBeacon(url, body)` — a
 *      locator text `"body"` at the call line resolves the argument
 *      IDENTIFIER, whose own declared type is `string` — the payload is one
 *      value-hop away through the declaration initializer.
 */

export interface CreatePaymentRequest {
  orderId: number;
  amountCents: number;
}

export interface MetricPayload {
  event: string;
  paymentId: string;
  durationMs: number;
}

// (a) createPayment-like: param, property name, and argument all named `body`.
export async function createPayment(body: CreatePaymentRequest): Promise<Response> {
  return fetch('/payments', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

// (b) beacon-like: a serialized identifier one hop from a typed param.
export function reportMetric(payload: MetricPayload): boolean {
  const body = JSON.stringify(payload);
  return navigator.sendBeacon('/metrics/ingest', body);
}

function getRawPayload(): string {
  return 'raw-payload';
}

// Control: an identifier declared from a NON-stringify initializer must not
// be rewritten by the one-hop declaration follow — it resolves whatever its
// own type is (here, faithfully `string`).
export function reportRaw(): boolean {
  const raw = getRawPayload();
  return navigator.sendBeacon('/metrics/raw', raw);
}
