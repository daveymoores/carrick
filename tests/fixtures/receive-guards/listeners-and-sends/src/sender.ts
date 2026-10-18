import { createSender } from "invented-app";

const sender = createSender();

export function finish(id: string): void {
  sender.send("DONE", { id });
}
