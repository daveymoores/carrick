// NATS pub/sub PUBLISHER — edge 3 CONSUMER (publisher = consumer of the topic).
//
// `nc.publish(SUBJECT, JSONCodec.encode(userRegistered))` emits the
// UserRegisteredEvent contract on the NATS subject `user.registered`. Publisher
// = consumer of the key; the matching producer (endpoint) is the SUBSCRIBER —
// notifications-svc `nc.subscribe('user.registered')`. Structured edge:
//   { producer_repo: notifications-svc (subscriber), consumer_repo: analytics-worker (publisher) }
// both on `pubsub|user.registered`. Broker (nats) is metadata, not key.
//
// Topic-literal variation: this site references a `const SUBJECT` (the Redis
// publisher uses an inline literal) — stresses the call-site literal resolver.

import { connect, JSONCodec } from "nats";
import type { UserRegisteredEvent } from "../types/metrics";

const SUBJECT = "user.registered";
const codec = JSONCodec<UserRegisteredEvent>();

export async function publishUserRegistered(
  userRegistered: UserRegisteredEvent
): Promise<void> {
  const nc = await connect({ servers: process.env.NATS_URL });
  // edge 3 consumer: const-subject reference, JSONCodec encode.
  nc.publish(SUBJECT, codec.encode(userRegistered));
}
