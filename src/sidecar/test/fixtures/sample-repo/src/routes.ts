/**
 * Sample routes file for integration testing
 *
 * Contains both explicitly typed and implicitly typed handlers
 * to test both bundle and infer functionality.
 */

import type { User } from './types.js';
import type { Order } from './models.js';

// Mock database for testing
const db = {
  findUser: async (id: string): Promise<User | null> => ({
    id,
    name: 'John Doe',
    email: 'john@example.com',
    createdAt: new Date(),
  }),
  findOrders: async (userId: string): Promise<Order[]> => [
    {
      id: 'order-1',
      userId,
      items: ['item-1', 'item-2'],
      totalCents: 9999,
      status: 'delivered',
      createdAt: new Date(),
    },
  ],
};

// Mock request/response types for testing
interface Request {
  params: Record<string, string>;
  query: Record<string, string>;
  body: unknown;
}

interface Response<T = unknown> {
  json: (data: T) => void;
  send: (data: T) => void;
  status: (code: number) => Response<T>;
}

// ===========================================================================
// EXPLICIT TYPES - For bundle testing
// ===========================================================================

/**
 * Handler with explicit Response type annotation
 * This should be detected as is_explicit=true
 */
export const getUser = async (
  req: Request,
  res: Response<User>
): Promise<void> => {
  const user = await db.findUser(req.params.id);
  if (!user) {
    res.status(404).json({ id: '', name: '', email: '', createdAt: new Date() } as User);
    return;
  }
  res.json(user);
};

/**
 * Explicitly typed handler returning Order[]
 */
export const getUserOrders = async (
  req: Request,
  res: Response<Order[]>
): Promise<void> => {
  const orders = await db.findOrders(req.params.userId);
  res.json(orders);
};

// ===========================================================================
// IMPLICIT TYPES - For inference testing
// ===========================================================================

/**
 * Handler WITHOUT explicit type annotation
 * TypeScript infers the return type from res.json(orders)
 * This should be detected as is_explicit=false
 */
export const getOrders = async (req: Request, res: Response) => {
  // Lots of setup code before the response
  const userId = req.params.userId;
  console.log('Fetching orders for user:', userId);

  // Validation
  if (!userId) {
    res.status(400).json({ error: 'Missing userId' });
    return;
  }

  // More logging
  console.log('Querying database...');

  // The actual data fetch - TypeScript knows this is Order[]
  const orders = await db.findOrders(userId);

  // Final response - type should be inferred as Order[]
  res.json(orders);
};

/**
 * Handler with multiple response types (union type inference)
 */
export const getOrderById = async (req: Request, res: Response) => {
  const { orderId } = req.params;

  if (!orderId) {
    // Error response type
    res.status(400).json({ error: 'Missing orderId', code: 'MISSING_ID' });
    return;
  }

  const orders = await db.findOrders('user-1');
  const order = orders.find((o) => o.id === orderId);

  if (!order) {
    // Different error response
    res.status(404).json({ error: 'Order not found', code: 'NOT_FOUND' });
    return;
  }

  // Success response - Order type
  res.json(order);
};

/**
 * Handler returning a direct value (Hono/modern framework style)
 */
export const getOrderCount = async (req: Request) => {
  const orders = await db.findOrders(req.params.userId);
  return { count: orders.length, userId: req.params.userId };
};

// ===========================================================================
// KOA-STYLE HANDLER - For ctx.body inference
// ===========================================================================

interface KoaContext {
  params: Record<string, string>;
  body: unknown;
  status: number;
}

/**
 * Koa-style handler using ctx.body assignment
 */
export const koaGetUser = async (ctx: KoaContext) => {
  const user = await db.findUser(ctx.params.id);
  if (!user) {
    ctx.status = 404;
    ctx.body = { error: 'User not found' };
    return;
  }
  ctx.body = user;
};
