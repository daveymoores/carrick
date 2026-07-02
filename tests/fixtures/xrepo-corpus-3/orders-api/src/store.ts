import { CreateOrder, OrderCreated, TimelineEvent } from "./types/orders";

export async function createOrder(order: CreateOrder): Promise<OrderCreated> {
  return {
    orderId: `ord_${order.customerId}`,
    status: "pending",
    total: { amount: 0, currency: "EUR" },
  };
}

export async function loadTimeline(orderId: string): Promise<TimelineEvent[]> {
  return [
    { at: "2026-07-01T09:00:00Z", status: "placed" },
    { at: "2026-07-01T10:00:00Z", status: "picked", note: `order ${orderId} left the shelf` },
  ];
}
