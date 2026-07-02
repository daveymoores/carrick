import ky from "ky";

const ORDERS_BASE = process.env.NEXT_PUBLIC_ORDERS_API_URL ?? "http://localhost:4003";

export interface NewOrder {
  customerId: string;
  items: { sku: string; quantity: number }[];
}

export interface OrderAck {
  orderId: string;
  status: "pending" | "confirmed";
  total: { amount: number; currency: string };
}

export async function placeOrder(order: NewOrder): Promise<OrderAck> {
  return ky.post(`${ORDERS_BASE}/orders`, { json: order }).json<OrderAck>();
}
