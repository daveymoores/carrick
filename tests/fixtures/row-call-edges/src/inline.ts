import type { ApiClient } from "./api-client.js";

export async function listInline(make: (projectId: string) => ApiClient) {
  const client = make("p1");
  return client.list();
}
