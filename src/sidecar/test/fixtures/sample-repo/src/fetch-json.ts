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
