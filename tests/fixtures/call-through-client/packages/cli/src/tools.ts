import { createClient, type Client } from "./api-client";

/** A tool is handed the client: the table's type gives its parameter one. */
interface Tool {
  name: string;
  handler: (client: Client, id: string) => Promise<unknown>;
}

export const tools: Tool[] = [{ name: "run", handler: (client, id) => client.getRun(id) }];

/** Runs the tool with the given name. */
export function runTool(name: string, id: string) {
  return tools.find((tool) => tool.name === name)?.handler(createClient(), id);
}
