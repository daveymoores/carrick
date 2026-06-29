// Redis pub/sub PUBLISHER — edge 4 CONSUMER (publisher = consumer of the topic).
//
// `redis.publish('metrics.page_view', JSON.stringify(pageView))` emits the
// PageView contract on the Redis channel `metrics.page_view`. In Carrick's
// pub/sub model a PUBLISHER is the CONSUMER (call) of the key it sends; the
// matching producer (endpoint) is the SUBSCRIBER on the other side — the
// web-dashboard `sub.subscribe('metrics.page_view')`. So the structured edge is
//   { producer_repo: web-dashboard (subscriber), consumer_repo: analytics-worker (publisher) }
// both on `pubsub|metrics.page_view`. Broker (redis) is detection metadata, not key.
//
// Topic-literal variation: this site uses an INLINE string literal
// ('metrics.page_view'); the NATS publisher uses a `const SUBJECT` reference.

import Redis from "ioredis";
import type { PageView } from "../types/metrics";

const redis = new Redis(process.env.REDIS_URL);

export async function publishPageView(pageView: PageView): Promise<void> {
  // edge 4 consumer: inline topic literal, JSON.stringify codec.
  await redis.publish("metrics.page_view", JSON.stringify(pageView));
}

// DECOY: ioredis is dual-use. These are key-value cache ops on a key that
// happens to share the string 'metrics.page_view' — NOT a pub/sub publish.
// The scanner must NOT hallucinate a topic from a cache key. Emits nothing.
export async function cacheLastView(pageView: PageView): Promise<void> {
  // DECOY — redis.set is key-value, not publish.
  await redis.set("metrics.page_view", JSON.stringify(pageView));
}

export async function readLastView(): Promise<string | null> {
  // DECOY — redis.get is key-value, not subscribe.
  return redis.get("metrics.page_view");
}
