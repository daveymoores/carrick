import { Socket } from "fixture-socket";

const socket = new Socket("wss://events.example");

export const USER_JOINED = "user.joined";
export const USER_LEFT = "user.left";

export function emit(name: string, data: unknown): void {
  socket.send(name, data);
}

export function joined(id: string): void {
  emit(USER_JOINED, { id });
}

export function left(id: string): void {
  emit(USER_LEFT, { id });
}
