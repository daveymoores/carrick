// DECOY — intra-repo dead-letter retry loop on `__dlq.retry`.
//
// This file BOTH publishes and subscribes to `__dlq.retry` within orders-engine
// (an internal self-edge). It must NOT surface as a cross-repo match: there is
// no other repo on this topic, and an intra-repo publisher↔subscriber pair is a
// self-loop, not a contract edge. Listed in expected.json `_must_not_emit`.
//
// Topic-literal variation: inline string literals here (producer.ts uses a
// `const TOPIC` reference).

import { Kafka } from "kafkajs";

const kafka = new Kafka({ clientId: "orders-engine-dlq", brokers: ["kafka:9092"] });

const dlqProducer = kafka.producer();
const dlqConsumer = kafka.consumer({ groupId: "orders-engine-dlq" });

// Publish a retry message back onto the internal DLQ topic (inline literal).
export async function requeue(raw: string): Promise<void> {
  await dlqProducer.send({
    topic: "__dlq.retry",
    messages: [{ value: raw }],
  });
}

// Subscribe to the SAME internal topic (inline literal) — intra-repo self-edge.
export async function drainDlq(): Promise<void> {
  await dlqConsumer.subscribe({ topic: "__dlq.retry", fromBeginning: true });
  await dlqConsumer.run({
    eachMessage: async ({ message }) => {
      const raw = message.value ? message.value.toString() : "";
      void raw;
    },
  });
}
