import { Kafka } from "kafkajs";
import { OrderStatusChanged } from "../types/orders";

// Mid-migration bridge: fulfillment publishes this topic on NATS; the mirrored
// Kafka topic is what this legacy subscriber still reads. Same topic string,
// different broker — the contract is the topic.
const kafka = new Kafka({ clientId: "orders-api", brokers: ["kafka:9092"] });
const consumer = kafka.consumer({ groupId: "orders-status" });

export async function runStatusSubscriber(): Promise<void> {
  await consumer.connect();
  await consumer.subscribe({ topic: "orders.status.changed" });
  await consumer.run({
    eachMessage: async ({ message }) => {
      if (!message.value) return;
      const evt = JSON.parse(message.value.toString("utf8")) as OrderStatusChanged;
      applyStatus(evt);
    },
  });
}

function applyStatus(evt: OrderStatusChanged): void {
  void evt;
}
