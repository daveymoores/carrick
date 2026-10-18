import { Channel } from "invented-ipc";

function pong(message: unknown): void {
  console.log("pong", message);
}

function halt(): void {
  process.exitCode = 0;
}

export const channel = new Channel({
  handlers: {
    PING: async (message: unknown) => pong(message),
    STOP: halt,
  },
});
