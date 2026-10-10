import { CreateOrder, Order } from "../types";

const orders: Order[] = [];

export function listOrders(): Order[] {
  return orders;
}

export function findOrder(id: string): Order | undefined {
  return orders.find((order) => order.id === id);
}

export function addOrder(input: CreateOrder): Order {
  const order: Order = { id: String(orders.length + 1), status: "open", ...input };
  orders.push(order);
  return order;
}
