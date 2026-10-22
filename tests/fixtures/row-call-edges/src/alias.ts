import type { MakeClient } from "./types.js";

export async function listAliased(make: MakeClient) {
  const client = make("p1");
  return client.list();
}
