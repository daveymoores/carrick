import { Kafka } from "kafkajs";
import type { OrderPlacedEvent } from "../types/events";

// Pub/sub SUBSCRIBER = contract PRODUCER for `pubsub|order.placed`.
// Edges 1 (orders-engine, compatible) + 2 (billing-svc, INCOMPATIBLE) both
// publish this topic; this is the single producer endpoint they join to.
//
// Topic-literal-via-const variation (one of the two required call sites):
// the topic is a `const TOPIC` reference, NOT an inline string literal — it
// stresses the scanner's const topic-literal resolver. The NATS subscriber
// (src/nats/subscriber.ts) uses the inline-literal form for contrast.
const TOPIC = "order.placed";

const kafka = new Kafka({ clientId: "notifications-svc", brokers: ["localhost:9092"] });
const consumer = kafka.consumer({ groupId: "notifications" });

export async function startOrderConsumer(): Promise<void> {
  await consumer.connect();
  // subscribe via the const reference — the topic-literal resolver must follow it.
  await consumer.subscribe({ topic: TOPIC, fromBeginning: false });

  await consumer.run({
    eachMessage: async ({ message }) => {
      // Codec unwrap: Kafka delivers a Buffer; decode to text then JSON.parse
      // into the subscriber contract type OrderPlacedEvent.
      const raw = message.value ? message.value.toString("utf8") : "{}";
      const event = JSON.parse(raw) as OrderPlacedEvent;
      console.log("order.placed", event.id, event.total.amountCents);
    },
  });
}
