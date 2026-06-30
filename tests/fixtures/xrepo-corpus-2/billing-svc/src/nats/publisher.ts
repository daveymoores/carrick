import { connect, JSONCodec } from "nats";
import type { PaymentCaptured } from "../types/billing";

// Topic supplied via a `const` reference (the kafka producer site uses an
// inline literal — together they stress the scanner's topic-literal resolver).
const TOPIC = "payment.captured";

const codec = JSONCodec<PaymentCaptured>();

// ORPHAN consumer (publisher = consumer): publishes NATS `payment.captured`.
// No subscriber exists anywhere in the corpus, so this edge must remain
// unmatched (orphan consumer on `pubsub|payment.captured`).
export async function publishPaymentCaptured(event: PaymentCaptured): Promise<void> {
  const nc = await connect({ servers: "nats://localhost:4222" });
  nc.publish(TOPIC, codec.encode(event));
  await nc.drain();
}
