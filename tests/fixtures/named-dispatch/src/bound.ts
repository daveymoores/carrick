import { bind, endpoint, eager } from "./remote";

function localList(): unknown[] {
  return [];
}

export const listOrders = bind(localList, "orders.bound", { timeoutMs: 1 });
export const getOrder = endpoint("/orders/:id");
export const warmed = eager("orders.warm");
