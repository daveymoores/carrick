import { announce } from "./session";

export function open() {
  announce("room:open");
}
