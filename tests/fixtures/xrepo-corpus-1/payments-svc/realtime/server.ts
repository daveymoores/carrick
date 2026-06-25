// Socket.IO server emit (producer of the event flow, #221).
//
// Scanner-detectable via src/socket_io.rs: a `new Server(...)` from "socket.io"
// makes `io` a server root; the `connection` handler's first param is a
// per-connection server socket; `socket.emit("payment:settled", ...)` is an
// EMITTER of the SERVER->CLIENT direction → keyed socket|SERVER->CLIENT|payment:settled.
//
// In Carrick's socket model an emitter is a CONSUMER (call) of the key it sends;
// the matching producer (endpoint) is the listener on the other side — the
// web-frontend client `socket.on("payment:settled", ...)`. So the structured
// cross-repo edge is { producer_repo: web-frontend (listener),
// consumer_repo: payments-svc (emitter) }, both on
// socket|SERVER->CLIENT|payment:settled. This mirrors the unit test
// `test_socket_matching_is_direction_aware` (client listener = endpoint).
//
// The emitted payload is the payments-svc `Payment` type (#222 carries it).

import { Server } from "socket.io";
import { createServer } from "http";
import type { Payment } from "../src/types";

const httpServer = createServer();
const io = new Server(httpServer);

io.on("connection", (socket) => {
  // Emit a settled payment to connected clients (server -> client).
  const settle = (payment: Payment) => {
    socket.emit("payment:settled", payment);
  };

  // A reserved lifecycle event — never becomes a contract event.
  socket.on("disconnect", () => {});

  // Demonstrate the emit on a tick (statically analyzable, not run).
  settle({
    id: "pay_1",
    orderId: 1,
    amountCents: 4999,
    status: "settled",
  });
});

httpServer.listen(4100);
