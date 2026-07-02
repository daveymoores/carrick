import { Server } from "socket.io";
import { SupportMessage } from "./types/tickets";

const io = new Server(4005);

// Server listener: the producer of the CLIENT->SERVER contract; the
// storefront's client emitter is the consumer.
io.on("connection", (socket) => {
  socket.on("support:message", (msg: SupportMessage) => {
    recordMessage(msg);
  });
});

function recordMessage(msg: SupportMessage): void {
  void msg;
}
