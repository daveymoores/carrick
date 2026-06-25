// Shared domain types for the orders package.
// The cross-repo eval expects this exact shape for Order.

export interface Order {
  id: number;
  amountCents: number;
  currency: string;
}

export interface StatusResponse {
  status: string;
  version: string;
  uptime: number;
}
