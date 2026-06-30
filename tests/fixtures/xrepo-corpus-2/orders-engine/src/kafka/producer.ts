// Kafka PUBLISHER of `order.placed` (edge 1). A publisher is the cross-repo
// CONSUMER (call) of the topic; the subscriber (notifications-svc) is the
// contract producer. Cross-repo key = `pubsub|order.placed` (no broker).
//
// The payload is wrapped in Envelope<OrderPlaced> at the call site (unwrap
// stress), but the contract type is the INNER OrderPlaced — that is the
// resolved_type in expected.json, not the envelope.
//
// Topic-literal variation: this site uses a `const TOPIC` reference (the DLQ
// decoy in dlq.ts uses inline string literals) to stress the literal resolver.

import { Kafka } from "kafkajs";
import type { OrderPlaced, Envelope } from "../types/order";

const TOPIC = "order.placed";

const kafka = new Kafka({ clientId: "orders-engine", brokers: ["kafka:9092"] });
const producer = kafka.producer();

export async function publishOrderPlaced(order: OrderPlaced): Promise<void> {
  const envelope: Envelope<OrderPlaced> = { v: 1, data: order };
  await producer.send({
    topic: TOPIC,
    messages: [{ value: JSON.stringify(envelope) }],
  });
}

// Static example payload (analyzable, not run).
void publishOrderPlaced({
  id: "ord_1",
  total: { amountCents: 4999, currency: "EUR" },
  note: "gift wrap",
});
