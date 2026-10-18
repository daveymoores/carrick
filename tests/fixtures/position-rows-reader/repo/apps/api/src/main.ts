import { BASE_PATH, ItemsClient, items } from "./client";

export async function load(client: ItemsClient): Promise<Response> {
  const url = new URL(BASE_PATH, "http://localhost");
  await fetch(url);
  await client.list();
  // @ts-expect-error: a name nothing declares.
  missingHelper(BASE_PATH);
  return items.list();
}
