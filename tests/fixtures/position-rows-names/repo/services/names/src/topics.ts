export const TOPICS = { orders: "orders.created", users: "users.created" } as const;
export const PREFIX = "svc";
// @ts-expect-error: a type nothing declares, so the value's type is the error type.
export const BROKEN: NoSuchType = "broken.topic";
