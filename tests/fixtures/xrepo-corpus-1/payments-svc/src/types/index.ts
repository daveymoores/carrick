// Shared domain types for payments-svc

export interface Payment {
  id: string;
  orderId: number;
  amountCents: number;
  status: "pending" | "settled";
}

export interface CreatePayment {
  orderId: number;
  amountCents: number;
}
