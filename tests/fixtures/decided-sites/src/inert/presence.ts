import { openSession } from "./session";

export function announce(): void {
  openSession("ws://chat").notify("user.joined", { at: 1 });
}
