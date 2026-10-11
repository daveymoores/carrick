import { EventEmitter } from "node:events";

export interface OrderPlaced {
  orderId: string;
  totalCents: number;
}

const bus = new EventEmitter();

bus.on("order.placed", (event: OrderPlaced) => {
  console.log(`order ${event.orderId} placed`);
});

export function announce(event: OrderPlaced): void {
  bus.emit("order.placed", event);
}
