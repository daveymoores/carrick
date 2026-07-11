import type { ApiResponse } from 'wrapper-lib';

// #336 live shape (carrick-demo-notification-service): the call is MULTI-LINE
// (argument on its own line, with a trailing comma) and the binding's last use
// is a scalar projection (`.data.length`), not the payload itself. Two things
// must survive that shape:
//  1. the locator must find the call from the LLM's compact single-line print
//     (the newline after `(` must not defeat exact matching), and
//  2. the anchor must be derived from the CALL's own payload
//     (`OrderData[]` → `OrderData` + depth 1), not from the terminal use
//     (`number`), which carries no symbol and no depth.
// Lives in its own fixture file — sidecar.test.ts hardcodes byte offsets into
// wrapper-usage.ts.
interface OrderData {
  id: number;
  userId: number;
  product: string;
  amount: number;
}

declare function apiGetOrders<T>(url: string): Promise<ApiResponse<T>>;

const ORDER_SERVICE_URL = 'http://localhost:3002';

export async function getOrderCount(): Promise<number> {
  const ordersResponse = await apiGetOrders<OrderData[]>(
    `${ORDER_SERVICE_URL}/api/orders`,
  );
  const orderCount = ordersResponse.data.length;
  return orderCount;
}

// #336 third path: the live locator is an SWC-shaped SPAN (1-based BytePos,
// so both ends sit one byte past the ts-morph 0-based offsets). Under strict
// containment that excludes the real call — the shifted end overshoots the
// call's end by one byte — and escalates to the smallest ENCLOSING call, the
// route registration, whose type anchors the router instead of the payload.
// Mirrors the live `notificationRouter.get("/status", async handler)`.
interface FakeRouter {
  get(path: string, handler: () => Promise<void>): FakeRouter;
}
declare const fakeRouter: FakeRouter;

fakeRouter.get('/status', async () => {
  const statusOrders = await apiGetOrders<OrderData[]>(
    `${ORDER_SERVICE_URL}/api/orders-status`,
  );
  console.log(statusOrders.data.length);
});

// #336 fourth path (the CI shape): the scanned checkout has NO node_modules —
// the demo workflow is checkout + carrick with no npm install — so the client
// library resolves to `any` and the call's SEMANTIC type carries nothing to
// anchor on. The caller's payload claim is still in the AST: the single
// explicit call generic (`axios.get<Order[]>`), whose type resolves against
// the repo's OWN sources regardless of the untyped client. `untypedAxios: any`
// mirrors what `import axios from 'axios'` resolves to without node_modules.
declare const untypedAxios: any;

fakeRouter.get('/orders-untyped', async () => {
  const ordersResponse = await untypedAxios.get<OrderData[]>(
    `${ORDER_SERVICE_URL}/api/orders-untyped`,
  );
  console.log(ordersResponse.data.length);
});
