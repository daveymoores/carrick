import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { ApiClient } from "./api-client.js";
import { createServer } from "./server.js";

function getRequiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    console.error(`Missing required environment variable: ${name}`);
    process.exit(1);
  }
  return value;
}

async function main() {
  const client = new ApiClient({
    apiEndpoint: getRequiredEnv("CARRICK_API_ENDPOINT"),
    apiKey: getRequiredEnv("CARRICK_API_KEY"),
    org: getRequiredEnv("CARRICK_ORG"),
  });

  const server = createServer(client);
  const transport = new StdioServerTransport();
  await server.connect(transport);

  // Log to stderr so it doesn't interfere with stdio JSON-RPC
  console.error("Carrick MCP server started");
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
