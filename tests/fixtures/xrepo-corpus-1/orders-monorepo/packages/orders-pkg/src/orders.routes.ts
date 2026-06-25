import fastify from 'fastify';
import type { Order, StatusResponse } from './types';

// Trap: renamed scoped-closure plugin param.
// The plugin callback's first argument is conventionally named after the instance
// (e.g. `app` or `instance`), but here it is renamed to `ordersRouter` — a child
// var name that is NOT the outer `app`. The scanner must resolve the owner as
// `ordersRouter`, the child variable that registers the route, NOT the outer param
// name that shadows it (silent-prefix-drop / owner-drift trap, issues #133/#167).
//
// This plugin is registered with prefix "/orders" so:
//   GET /:id   → GET /orders/:id
//   GET /      → (status endpoint registered separately below)

const ordersPlugin = async (ordersRouter: ReturnType<typeof fastify>, opts: { prefix?: string }): Promise<void> => {
  // GET /orders/:id — response type: Order
  // Owner must be `ordersRouter` (the renamed scoped-closure param), not `app`.
  ordersRouter.get<Order>('/:id', async (request, reply): Promise<Order> => {
    const { id } = request.params;
    const order: Order = {
      id: Number(id),
      amountCents: 4999,
      currency: 'EUR',
    };
    return order;
  });
};

export { ordersPlugin };
