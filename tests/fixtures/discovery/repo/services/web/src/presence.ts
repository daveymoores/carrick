import { openSession } from "./session";

export function announce(): void {
  openSession("wss://chat.example").notify("user.joined", { at: 1 });
}
