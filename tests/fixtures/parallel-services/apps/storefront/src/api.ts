import { OrderView, ProductView } from "./models";

const ordersBase = process.env.ORDERS_API_URL;
const catalogBase = process.env.CATALOG_API_URL;

export async function loadOrders(): Promise<OrderView[]> {
  const res = await fetch(`${ordersBase}/api/orders`);
  const orders: OrderView[] = await res.json();
  return orders;
}

export async function loadOrder(id: string): Promise<OrderView> {
  const res = await fetch(`${ordersBase}/api/orders/${id}`);
  const order: OrderView = await res.json();
  return order;
}

export async function loadProducts(): Promise<ProductView[]> {
  const res = await fetch(`${catalogBase}/products`);
  const products: ProductView[] = await res.json();
  return products;
}
