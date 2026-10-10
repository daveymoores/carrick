export interface OrderView {
  id: string;
  customerId: string;
  totalCents: number;
  status: string;
}

export interface ProductView {
  sku: string;
  title: string;
  price: string;
}
