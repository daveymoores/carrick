import { fork } from "node:child_process";

function parseMessage(raw: unknown): { type: string } {
  return JSON.parse(String(raw));
}

export function startWorker(script: string): void {
  const child = fork(script);
  child.on("message", (msg: { type: string }) => {
    switch (msg.type) {
      case "READY":
        child.send({ type: "START" });
        break;
      case "DONE":
        child.kill();
        break;
    }
  });
  child.on("message", (raw: unknown) => {
    const msg = parseMessage(raw);
    if (msg.type === "FAILED") {
      child.kill();
    }
  });
}
