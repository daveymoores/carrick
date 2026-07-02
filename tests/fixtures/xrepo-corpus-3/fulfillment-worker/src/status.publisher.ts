import { connect, JSONCodec } from "nats";
import { OrderStatusChanged } from "./types/fulfillment";

const jc = JSONCodec<OrderStatusChanged>();

// Publishes on NATS; orders-api still reads the mirrored Kafka topic during
// the broker migration. Same topic string, different broker.
export async function publishStatusChanged(evt: OrderStatusChanged): Promise<void> {
  const nc = await connect({ servers: process.env.NATS_URL });
  nc.publish("orders.status.changed", jc.encode(evt));
  await nc.close();
}
