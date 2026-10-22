import { http } from '@fixture/http';

export interface Order {
  sku: string;
  quantity: number;
}

export interface Receipt {
  orderId: string;
  total: number;
}

export async function placeOrder(order: Order): Promise<Receipt> {
  return http.post<Receipt>('/orders', order);
}
