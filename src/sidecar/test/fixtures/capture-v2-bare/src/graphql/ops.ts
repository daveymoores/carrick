import type { OrderResponse } from '../http/routes';

export interface OrderQueryResult {
  order: OrderResponse | null;
}
