import { Kafka } from "kafkajs";
import type { OrderPlaced } from "../types/billing";

const kafka = new Kafka({ clientId: "billing-svc", brokers: ["localhost:9092"] });
const producer = kafka.producer();

// EDGE 2 (consumer = publisher): publishes Kafka `order.placed`.
// Topic supplied as an INLINE string literal (the other pub/sub site,
// nats/publisher.ts, uses a `const TOPIC` reference — stresses the literal
// resolver). Payload is INCOMPATIBLE with the subscriber contract: OrderPlaced
// here carries `total: number`, the subscriber expects a nested Money object.
export async function publishOrderPlaced(order: OrderPlaced): Promise<void> {
  await producer.connect();
  await producer.send({
    topic: "order.placed",
    messages: [{ key: order.id, value: JSON.stringify(order) }],
  });
}
