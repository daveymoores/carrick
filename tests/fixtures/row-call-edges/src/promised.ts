import type { OpenClient } from "./types.js";

export async function listAwaited(open: OpenClient) {
  const client = await open("p1");
  return client.list();
}

export async function listUnawaited(open: OpenClient) {
  const client = open("p1");
  // @ts-expect-error a promise of the client has no `list`
  return client.list();
}
