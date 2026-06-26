export async function loadUser() {
  const response = await fetch('https://example.com/users/1');
  const user: { id: string; name: string } = await response.json();
  return user;
}

// #257: cross-repo consumer shape. An `as Promise<NamedInterface>` cast on a
// json() call is the exact pattern that produced a dangling `= OrderView`
// alias in the bundle. The named interface must be recovered STRUCTURALLY so
// the cross-repo .d.ts carries the real members
// (`{ id: string; currency: string }`), not a name that resolves to `any`.
export interface OrderView {
  id: string;
  currency: string;
}

export async function loadOrder(res: Response) {
  return res.json() as Promise<OrderView>;
}

// #257/#240: PRODUCER-side structural expansion + deterministic anchor.
// A producer emitting a payload typed `Payment` used to infer the bare name
// `Payment`, which dangles in the source-less cross-repo bundle as
// `export type <alias> = Payment;` → resolves to `any` → unverifiable →
// `compat = None`. The payload must be expanded STRUCTURALLY to its members,
// and the inferred type must carry `primary_type_symbol: "Payment"` so the
// manifest anchor no longer depends on the LLM.
export interface Payment {
  id: string;
  amountCents: number;
  currency: string;
}

interface ResLike {
  json: (data: unknown) => void;
}

export function sendPayment(res: ResLike, payment: Payment) {
  res.json(payment);
}
