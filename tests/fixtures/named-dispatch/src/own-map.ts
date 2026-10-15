import type { Handler } from "./registry";
import { publish } from "rpc-mount";

class Map {
  set(name: string, handler: Handler): void {
    publish(name, handler);
  }
}

const table = new Map();

export function own(name: string, handler: Handler): void {
  table.set(name, handler);
}
