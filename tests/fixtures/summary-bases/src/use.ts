import { client } from "./client";
import { movingClient, paramClient } from "./injected";
import { getThings, ping } from "./things";

export async function load(): Promise<void> {
  await client.list();
  await paramClient.items();
  await movingClient.items();
  await getThings();
  await ping();
}
