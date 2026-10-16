import type { Gateway } from "./gateway.js";

export async function runOps(gateway: Gateway, q: string): Promise<void> {
  await gateway.purgeAll({});
  await gateway.refreshBoth();
  await gateway.search({ q });
  await gateway.lock({ op: "unlock" });
}

// Nothing calls this, so nothing fills `given`.
export async function passOn(gateway: Gateway, given: object): Promise<unknown> {
  return gateway.purgeAll(given);
}
