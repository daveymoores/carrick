import server from 'fixture-plugin-server';
import { orderRoutes } from './orders';

// The package also builds an app of its own for local runs, serving its
// order routes at the root, so it uses the server package as a value and
// not only through its types.
export function standalone() {
  const app = server();
  app.register(orderRoutes);
  return app;
}
