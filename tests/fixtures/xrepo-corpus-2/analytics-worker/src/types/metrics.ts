// Publisher payload types for analytics-worker.
// These are the CONSUMER-side contract shapes (publisher = consumer of the topic).
// They must stay byte-compatible with the subscriber/producer answer key:
//   - PageView           → web-dashboard subscriber (Redis metrics.page_view)
//   - UserRegisteredEvent → notifications-svc subscriber (NATS user.registered)

export interface PageView {
  path: string;
  userId: string;
  ts: number;
}

export interface UserRegisteredEvent {
  userId: string;
  email: string;
  registeredAt: number;
}
