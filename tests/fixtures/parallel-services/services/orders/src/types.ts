export interface Order {
  id: string;
  customerId: string;
  totalCents: number;
  status: "open" | "paid" | "shipped";
}

export interface Customer {
  id: string;
  name: string;
  email: string;
}

export interface Invoice {
  id: string;
  orderId: string;
  amount: string;
}

export interface CreateOrder {
  customerId: string;
  totalCents: number;
}
