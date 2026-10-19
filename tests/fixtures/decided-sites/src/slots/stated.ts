import { useCached } from "cache-kit";
import { link } from "event-kit";

export function readX() {
  return useCached({ key: ["x"], load: () => fetch("/api/x") });
}

export function readGiven(options) {
  return useCached(options);
}

export function readSpread(options) {
  return useCached({ ...options });
}

const conn = link("ws://x");

export function listen(): void {
  conn.on("open", () => {});
  conn.on("chat", () => {});
  conn.once("close", () => {});
}
