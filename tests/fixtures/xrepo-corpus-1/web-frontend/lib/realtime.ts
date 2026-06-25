// Socket.IO client listener (consumer side of the event flow, #221).
//
// Scanner-detectable via src/socket_io.rs: `io(...)` from "socket.io-client"
// makes `socket` a client socket; `socket.on("payment:settled", ...)` is a
// LISTENER of the SERVER->CLIENT direction → keyed
// socket|SERVER->CLIENT|payment:settled.
//
// In Carrick's socket model a listener is a PRODUCER (endpoint) of the key it
// receives. So web-frontend owns the producer side of this edge; the matching
// consumer (call) is the payments-svc server emit. Both meet on
// socket|SERVER->CLIENT|payment:settled.
//
// The payload shape (SettledPayment) is compatible with the payments-svc
// Payment producer type.

import { io } from "socket.io-client";

const PAYMENTS_WS = process.env.NEXT_PUBLIC_PAYMENTS_WS_URL ?? "";
const socket = io(PAYMENTS_WS);

// Consumer view of the settled-payment payload. Compatible with payments-svc
// `Payment` (id/orderId/amountCents/status).
export interface SettledPayment {
  id: string;
  orderId: number;
  amountCents: number;
  status: string;
}

// Listen for server -> client `payment:settled` events.
export function onPaymentSettled(handler: (p: SettledPayment) => void): void {
  socket.on("payment:settled", (payload: SettledPayment) => {
    handler(payload);
  });
}

// Reserved lifecycle event — never a contract event.
socket.on("connect", () => {});
