import { client } from "./client";

// A call through a client the file does not define. The text of this file
// never names an environment variable or a base: the extraction model stated
// one for the call, and the file gives it nothing to stand on.
export async function planSummary(team: string): Promise<string> {
  const result = await client.getSummary(team);
  return result;
}
