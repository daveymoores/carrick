import type { Deps } from "./types.js";

export async function removeThrough({ make }: Deps, id: string) {
  const client = make("p1");
  await client.remove(id);
}

export async function listUntyped(make: Function) {
  const client = make("p1");
  return client.list();
}
