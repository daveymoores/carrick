// Barrel re-export: handlers and types surface through this index.
// Trap: re-exported through a barrel. The scanner must follow the re-export
// chain to attribute endpoints to their real authoring module, not this barrel.

export { ordersPlugin } from './orders.routes';
export { statusRouter } from './status.routes';
export type { Order, StatusResponse } from './types';
export { default as app } from './server';
