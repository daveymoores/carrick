import type { Item } from '@app/models/item';

export interface OrderResponse {
  id: string;
  items: Item[];
  total: number;
}

export interface CreateOrderRequest {
  items: Item[];
  couponCode?: string;
}

export async function getOrder(id: string): Promise<OrderResponse> {
  return { id, items: [], total: 0 };
}

export function overloadedHandler(a: string): OrderResponse;
export function overloadedHandler(a: number): OrderResponse;
export function overloadedHandler(a: unknown): OrderResponse {
  return { id: String(a), items: [], total: 0 };
}

export function genericHandler<T>(x: T): { data: T } {
  return { data: x };
}
