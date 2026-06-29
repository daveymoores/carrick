// Redis pub/sub SUBSCRIBER (ioredis). In Carrick's pub/sub model a subscriber is
// the contract PRODUCER (endpoint) of the topic it receives, mirroring the
// socket-listener-as-producer rule. So web-dashboard owns the producer side of
// `pubsub|metrics.page_view`; the matching consumer (call) is analytics-worker's
// redis.publish. Both meet on the topic literal, NOT on a connection URL — so no
// internalEnvVars entry is needed for this edge.
//
//   sub.subscribe('metrics.page_view') + sub.on('message', JSON.parse) -> pubsub|metrics.page_view [edge 4 PRODUCER]

import Redis from "ioredis";

const REDIS_URL = process.env.REDIS_URL ?? "";
const sub = new Redis(REDIS_URL);

// Subscriber payload — compatible with the analytics-worker publisher PageView.
export interface PageView {
  path: string;
  userId: string;
  ts: number;
}

// Subscribe to the page-view stream and decode each message body.
// inline topic literal 'metrics.page_view' (web-dashboard's single pub/sub site;
// the const-vs-inline literal variation is exercised on the broker repos).
export function onPageView(handler: (view: PageView) => void): void {
  sub.subscribe("metrics.page_view");
  sub.on("message", (_channel: string, message: string) => {
    const view = JSON.parse(message) as PageView;
    handler(view);
  });
}
