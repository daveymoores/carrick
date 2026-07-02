import { io } from "socket.io-client";

const socket = io(process.env.NEXT_PUBLIC_SUPPORT_WS_URL ?? "http://localhost:4005");

export interface SupportMessage {
  ticketId: string;
  body: string;
  sentAt: string;
}

// Client emitter: the CLIENT->SERVER contract consumer; support-desk's server
// listener is the producer.
export function sendSupportMessage(msg: SupportMessage): void {
  socket.emit("support:message", msg);
}
