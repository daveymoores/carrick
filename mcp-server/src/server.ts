import { McpServer, ResourceTemplate } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { ApiClient } from "./api-client.js";
import { listServices } from "./tools/list-services.js";
import { getEndpoints } from "./tools/get-endpoints.js";
import { getEndpointTypes } from "./tools/get-types.js";
import { checkCompatibility } from "./tools/check-compat.js";
import { getServiceDependencies } from "./tools/get-deps.js";
import { getServiceCatalog } from "./resources/service-catalog.js";
import { getServiceTypes } from "./resources/service-types.js";

export function createServer(client: ApiClient): McpServer {
  const server = new McpServer({
    name: "carrick",
    version: "0.1.0",
  });

  // --- Tools ---

  server.tool(
    "list_services",
    "List all services in the org with endpoint counts, call counts, and type availability",
    {},
    async () => listServices(client),
  );

  server.tool(
    "get_api_endpoints",
    "Get the API endpoints exposed by a service. Returns method, path, handler, owner, and file location.",
    {
      service: z.string().describe("Service name to look up (fuzzy match by repo name, service name, or trailing segment)"),
      method: z.string().optional().describe("Filter by HTTP method (GET, POST, etc.)"),
      path_contains: z.string().optional().describe("Filter endpoints whose path contains this substring"),
    },
    async (params) => getEndpoints(client, params),
  );

  server.tool(
    "get_endpoint_types",
    "Get the request/response TypeScript type definitions for a specific API endpoint. This is the highest-value tool — returns extracted .d.ts types.",
    {
      service: z.string().describe("Service name"),
      method: z.string().describe("HTTP method (GET, POST, PUT, DELETE)"),
      path: z.string().describe("API path (e.g. /api/users/:id)"),
    },
    async (params) => getEndpointTypes(client, params),
  );

  server.tool(
    "check_compatibility",
    "Check if a consumer's API calls are compatible with a producer's endpoints. Finds missing endpoints and unused endpoints.",
    {
      consumer_service: z.string().describe("The service making API calls"),
      producer_service: z.string().describe("The service exposing endpoints"),
      method: z.string().optional().describe("Filter by HTTP method"),
      path: z.string().optional().describe("Filter by specific path"),
    },
    async (params) => checkCompatibility(client, params),
  );

  server.tool(
    "get_service_dependencies",
    "Get package dependencies for a service, or find version conflicts across all services in the org.",
    {
      service: z.string().optional().describe("Service name (omit for org-wide conflict analysis)"),
    },
    async (params) => getServiceDependencies(client, params),
  );

  // --- Resources ---

  server.resource(
    "service-catalog",
    "carrick://services",
    { description: "Service catalog listing all services in the org" },
    async (uri) => ({
      contents: [
        {
          uri: uri.href,
          mimeType: "application/json",
          text: await getServiceCatalog(client),
        },
      ],
    }),
  );

  server.resource(
    "service-types",
    new ResourceTemplate("carrick://services/{name}/types.d.ts", { list: undefined }),
    { description: "Full bundled TypeScript type definitions for a service" },
    async (uri, variables) => {
      const name = variables.name as string;
      return {
        contents: [
          {
            uri: uri.href,
            mimeType: "text/typescript",
            text: await getServiceTypes(client, name),
          },
        ],
      };
    },
  );

  return server;
}
