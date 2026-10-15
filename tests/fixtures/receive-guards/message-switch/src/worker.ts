import { MessageKind } from "./wire";
import { Signals } from "invented-protocol";

function pause(): void {
  setTimeout(() => undefined, 1000);
}

process.on("message", (msg: { kind: string }) => {
  if (msg.kind === MessageKind.STOP) {
    process.exit(0);
  }
  if (msg.kind === Signals.PAUSE) {
    pause();
  }
});
