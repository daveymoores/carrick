import { Order } from './types';

async function getJson(url: string): Promise<unknown> {
  const resp = await fetch(url);
  return resp.json();
}

export async function fetchOrders(userId: number): Promise<Order[]> {
  const orders = (await getJson(
    `https://orders.internal/api/orders?user=${userId}`
  )) as Order[];
  return orders;
}
