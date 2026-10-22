import type { Client } from "fixture-fetcher";

// An HTTP client handed in as a parameter: found by its declaration too.
export function load(client: Client): Promise<unknown> {
  return client.get("/items");
}
