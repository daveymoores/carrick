// Next.js App Router page: app/orders/[id]/page.tsx → /orders/:id
// Uses lib/api.ts to fetch the order and render it.
// The [id] param is a Next.js dynamic-route segment, canonicalised to :param.

import { fetchOrder, createPayment, type OrderView, type Payment } from "../../../lib/api";

interface PageProps {
  params: { id: string };
}

export default async function OrderPage({ params }: PageProps) {
  const order: OrderView = await fetchOrder(params.id);

  return (
    <main>
      <h1>Order {order.id}</h1>
      <p>Currency: {order.currency}</p>
    </main>
  );
}

// Re-export types so the scanner sees them referenced in this file too.
export type { OrderView, Payment };
