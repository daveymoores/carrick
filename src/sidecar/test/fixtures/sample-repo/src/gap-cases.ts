/**
 * Gap-regression fixture: realistic handler shapes that exercise locator and
 * unwrapping edge cases. Each section is paired with a test in
 * test/gap-regression.test.ts that sends the same locator payload the
 * analysis stage would emit (expression text + line), including drifted
 * line numbers.
 *
 * Line numbers are load-bearing — tests reference them. If you edit this
 * file, update the constants at the top of gap-regression.test.ts.
 */

export interface User {
  id: number;
  name: string;
}

export interface CachedUsers {
  users: User[];
  fetchedAt: string;
}

// --- Substring locator: `users` must not bind to `usersCsv` ---
// The real payload (`users`) sits outside the ±5-line search window around
// the drifted line the analysis reported; the only nearby identifier that
// contains the text is `usersCsv`.
export function exportUsersHandler(
  _req: unknown,
  res: { json: (v: unknown) => void; setHeader: (k: string, v: string) => void }
): void {
  const usersCsv: string = 'id,name';
  res.setHeader('content-type', 'text/csv');
  // filler so the real payload is outside the search radius
  // .
  // .
  // .
  // .
  // .
  const users: User[] = [{ id: 1, name: 'Alice' }];
  res.json(users);
}

// --- Payload-less imperative handler: response type must not become `void` ---
export function redirectHandler(
  _req: unknown,
  res: { redirect: (url: string) => void }
): void {
  res.redirect('/login');
}

// --- Return-value handler: the same fallback must keep working here ---
export async function returnStyleHandler(): Promise<User> {
  return { id: 2, name: 'Bob' };
}

// --- Union of Promises: each member must unwrap ---
async function fetchUsers(): Promise<User[]> {
  return [{ id: 1, name: 'Alice' }];
}

async function readCache(): Promise<CachedUsers> {
  return { users: [], fetchedAt: 'never' };
}

export function loadUsers(useCache: boolean) {
  return useCache ? readCache() : fetchUsers();
}

// --- Registration-call span: must not type the route path literal ---
// When the analysis has no payload expression it falls back to the SWC span
// of the endpoint *registration* call. Drilling into that call's first
// argument would yield the path string literal as the "response type".
interface RedirectRes {
  redirect: (url: string) => void;
}

export const app = {
  get(_path: string, _handler: (req: unknown, res: RedirectRes) => void): void {},
};

app.get('/login-redirect', (_req, res) => {
  res.redirect('/login');
});
