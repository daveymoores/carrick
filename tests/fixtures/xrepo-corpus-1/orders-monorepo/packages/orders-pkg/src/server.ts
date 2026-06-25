import fastify from 'fastify';
import { ordersPlugin } from './orders.routes';
import { statusRouter } from './status.routes';

const app = fastify();

// Register the orders plugin with a "/orders" prefix.
// Routes inside the plugin:
//   GET /:id  → resolves to GET /orders/:id
app.register(ordersPlugin, { prefix: '/orders' });

// Register the statusRouter plugin.
// statusRouter carries its own prefix ("/api/v1") in its constructor;
// mount_path is "" (no additional path segment here).
// The scanner must recognise the constructor-carried prefix.
app.register(async (instance) => {
  for (const route of statusRouter.routes()) {
    // Re-attach routes from the prefix-carrying router under its own prefix.
    // The effective path becomes statusRouter.prefix + route.path = /api/v1/status.
    (instance as any)[route.method.toLowerCase()](
      statusRouter.prefix + route.path,
      route.handler
    );
  }
}, { prefix: '' });

export default app;
