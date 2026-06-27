/**
 * Fixture for the request_body structural-expansion regression.
 *
 * Mirrors the cross-repo corpus `POST /payments` edge: a producer that casts
 * `req.body` to a named request type, and a consumer that `JSON.stringify`s a
 * named-typed payload into a `fetch` body. Both used to infer the BARE type
 * NAME, which dangles in the source-less cross-repo `.d.ts` bundle (alias lines
 * only) → resolves to `any` → `unverifiable`. The fix expands the resolved
 * named request type to its STRUCTURAL member shape so the real members reach
 * the bundle.
 */

export interface CreatePayment {
  orderId: number;
  amountCents: number;
}

interface ReqLike {
  body: unknown;
}

// Producer: the request body is recovered from an explicit `as CreatePayment`
// cast on `req.body` (whose own type is `unknown`). The cast TARGET must be
// expanded structurally, not emitted as the bare name `CreatePayment`.
export function createPaymentHandler(req: ReqLike) {
  const body = req.body as CreatePayment;
  return body.orderId;
}

// Consumer: the `fetch` POST body serializes a `payload: CreatePayment`.
// Drilling into the serializer yields the named `CreatePayment`; its resolved
// shape must be expanded structurally, not left as the dangling bare name.
export async function sendCreatePayment(payload: CreatePayment) {
  return fetch('/payments', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
}
