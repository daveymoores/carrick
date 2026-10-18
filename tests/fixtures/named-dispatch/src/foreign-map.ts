import type { Handler } from "./registry";
import { Map } from "rpc-collections";

const table = new Map<string, Handler>();

export function foreign(name: string, handler: Handler): void {
  table.set(name, handler);
}
