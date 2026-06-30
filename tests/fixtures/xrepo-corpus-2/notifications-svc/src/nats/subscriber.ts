import { connect, StringCodec } from "nats";
import type { UserRegisteredEvent } from "../types/events";

// Pub/sub SUBSCRIBER = contract PRODUCER for `pubsub|user.registered`.
// Edge 3: analytics-worker publishes this topic (compatible). This is the
// producer endpoint it joins to.
//
// Topic-literal INLINE-LITERAL variation (the second of the two required call
// sites): the subject is passed as a bare string literal `"user.registered"`
// directly to nc.subscribe(...) — contrast with the const-ref Kafka subscriber.
//
// Async-iterator subscribe: `for await (const m of sub)` drains the
// Subscription's AsyncIterable; NATS delivers a Uint8Array (m.data) decoded
// via StringCodec then JSON.parse into the contract type.
export async function startUserConsumer(): Promise<void> {
  const nc = await connect({ servers: "localhost:4222" });
  const sc = StringCodec();

  const sub = nc.subscribe("user.registered");

  for await (const m of sub) {
    const event = JSON.parse(sc.decode(m.data)) as UserRegisteredEvent;
    console.log("user.registered", event.userId, event.email, event.registeredAt);
  }
}
