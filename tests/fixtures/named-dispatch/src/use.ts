import { registerNamed, dispatch } from "./registry";
import type { Handler } from "./registry";
import { endpoint } from "./remote";

const readOrder: Handler = (input) => input;

export async function boot(): Promise<void> {
  await registerNamed("orders.boot", readOrder);
}

export async function run(): Promise<unknown> {
  return await dispatch("orders.run");
}

export function lines() {
  return endpoint("/orders/:id/lines");
}
