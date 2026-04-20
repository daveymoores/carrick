/**
 * Framework-idiomatic response handlers, one file per shape.
 *
 * These exercise the Move 1 invariant: the LLM emits the payload subexpression
 * directly (e.g., `users`), the sidecar resolves the node by text and reads
 * its type — no method-name whitelist, no drilling. Works identically across
 * res.json / ctx.body = / h.response / bare return.
 */

export interface User {
  id: number;
  name: string;
}

export interface Order {
  id: number;
  total: number;
}

// 1. Express: res.json(users)
export function expressHandler(_req: unknown, res: { json: (v: unknown) => void }): void {
  const users: User[] = [{ id: 1, name: 'Alice' }];
  res.json(users);
}

// 2. Koa: ctx.body = users
export async function koaHandler(ctx: { body?: unknown }): Promise<void> {
  const users: User[] = [{ id: 1, name: 'Alice' }];
  ctx.body = users;
}

// 3. Hapi: return h.response(order)
export async function hapiHandler(_request: unknown, h: { response: <T>(v: T) => T }): Promise<Order> {
  const order: Order = { id: 1, total: 100 };
  return h.response(order);
}

// 4. Fastify/NestJS/Hono: bare return
export async function bareReturnHandler(): Promise<User> {
  const user: User = { id: 2, name: 'Bob' };
  return user;
}
