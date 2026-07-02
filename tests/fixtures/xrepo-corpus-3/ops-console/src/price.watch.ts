import { connect, StringCodec } from "nats";

const sc = StringCodec();

// Fan-out subscriber B (inventory-svc is A). effectiveAt is optional here
// while the publisher always sends it — safe required->optional narrowing,
// this edge is compatible.
export interface PriceAlert {
  productId: string;
  price: { amount: number; currency: string };
  effectiveAt?: string;
}

export async function watchPriceAlerts(): Promise<void> {
  const nc = await connect({ servers: process.env.NATS_URL });
  for await (const msg of nc.subscribe("catalog.price.updated")) {
    const alert = JSON.parse(sc.decode(msg.data)) as PriceAlert;
    notifyOps(alert);
  }
}

function notifyOps(alert: PriceAlert): void {
  void alert;
}
