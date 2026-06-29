// Subscriber payload contract types — this repo is the CONTRACT PRODUCER
// for the topics it subscribes to (pub/sub direction rule: subscriber = producer).
// The cross-repo eval expects these EXACT shapes; they are the join-key answer.

// Nested money object — stresses the codec-unwrap + nested-field compat path.
// Compatible publisher: orders-engine OrderPlaced.total = { amountCents; currency }.
// Incompatible publisher: billing-svc OrderPlaced.total = bare number.
export interface Money {
  amountCents: number;
  currency: string;
}

// Kafka "order.placed" subscriber payload (edges 1 + 2 producer side).
export interface OrderPlacedEvent {
  id: string;
  total: Money;
  note?: string;
}

// NATS "user.registered" subscriber payload (edge 3 producer side).
export interface UserRegisteredEvent {
  userId: string;
  email: string;
  registeredAt: number;
}
