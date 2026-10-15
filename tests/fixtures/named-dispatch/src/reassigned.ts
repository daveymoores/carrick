import type { Handler } from "./registry";

let table = new Map<string, Handler>();

export function reset(): void {
  table = new Map();
}

export function swap(name: string, handler: Handler): void {
  table.set(name, handler);
}
