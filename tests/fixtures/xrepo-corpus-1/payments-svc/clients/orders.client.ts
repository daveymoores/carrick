import axios from "axios";

// Response shape compatible with orders-monorepo's Order producer
export interface OrderResponse {
  id: number;
  amountCents: number;
  currency: string;
}

const ORDERS_BASE = process.env.ORDERS_SERVICE_URL ?? "http://localhost:3001";

export async function getOrder(orderId: number): Promise<OrderResponse> {
  const response = await axios.get<OrderResponse>(
    `${ORDERS_BASE}/orders/${orderId}`
  );
  return response.data;
}
