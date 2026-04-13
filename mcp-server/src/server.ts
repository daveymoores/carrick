import { McpServer, ResourceTemplate } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { ApiClient } from "./api-client.js";
import { listServices } from "./tools/list-services.js";
import { getEndpoints } from "./tools/get-endpoints.js";
import { getEndpointTypes } from "./tools/get-types.js";
import { checkCompatibility } from "./tools/check-compat.js";
import { getServiceDependencies } from "./tools/get-deps.js";
import { getTypeDefinition } from "./tools/get-type-definition.js";
import { listFunctionIntents } from "./tools/find-similar.js";
import { getFunctionSource } from "./tools/get-function-source.js";
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
    "List all services in the organization with endpoint counts, API call counts, and whether TypeScript types are available. Use this first to discover what services exist before drilling into specific endpoints or types.",
    {},
    async () => listServices(client),
  );

  server.tool(
    "get_api_endpoints",
    "Get the API endpoints exposed by a single service. Returns method, path, handler, owner, and file location for each endpoint. Use this to explore what routes a service exposes before checking types or compatibility.",
    {
      service: z.string().describe("Service name to look up (fuzzy match by repo name, service name, or trailing segment)"),
      method: z.string().optional().describe("Filter by HTTP method (GET, POST, etc.)"),
      path_contains: z.string().optional().describe("Filter endpoints whose path contains this substring"),
    },
    async (params) => getEndpoints(client, params),
  );

  server.tool(
    "get_endpoint_types",
    "Get the TypeScript request/response type definitions for a specific API endpoint. Returns extracted .d.ts types including whether they were explicitly annotated or inferred. Use get_api_endpoints first to find the exact method and path.",
    {
      service: z.string().describe("Service name"),
      method: z.string().describe("HTTP method (GET, POST, PUT, DELETE)"),
      path: z.string().describe("API path (e.g. /api/users/:id)"),
    },
    async (params) => getEndpointTypes(client, params),
  );

  server.tool(
    "check_compatibility",
    "Check if a specific consumer service's API calls are compatible with a specific producer service's endpoints. Compares one consumer-producer pair — reports missing endpoints (consumer calls something the producer doesn't expose) and unused endpoints. Use list_services first to find service names.",
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
    "Get npm package dependencies for a single service, or omit the service parameter to find version conflicts across all services in the organization. Useful for detecting mismatched dependency versions that could cause runtime issues.",
    {
      service: z.string().optional().describe("Service name (omit for org-wide conflict analysis)"),
    },
    async (params) => getServiceDependencies(client, params),
  );

  server.tool(
    "get_type_definition",
    "Get the full resolved TypeScript definition for a named type, including all transitive dependencies. Use this after get_endpoint_types to get complete type definitions.",
    {
      service: z.string().describe("Service name"),
      type_alias: z.string().describe("Type alias name (from get_endpoint_types results)"),
    },
    async (params) => getTypeDefinition(client, params),
  );

  server.tool(
    "list_function_intents",
    "List all exported functions with their LLM-generated intent descriptions across org services. Returns a compact list the agent can scan to discover existing implementations. Use gh CLI to browse the actual source code on GitHub.",
    {
      service: z.string().optional().describe("Filter to a specific service (omit for all services)"),
      exclude_service: z.string().optional().describe("Service to exclude from results"),
    },
    async (params) => listFunctionIntents(client, params),
  );

  server.tool(
    "get_function_source",
    "Get structured metadata and GitHub fetch coordinates for a specific function. Returns route, resolved request/response types, external API calls, internal calls, and intent. Use detail='signature' for metadata only, or detail='full' (default) to also get line-precise GitHub coordinates for fetching just the function source. Use list_function_intents first to find function names.",
    {
      service: z.string().describe("Service name (fuzzy match)"),
      function_name: z.string().describe("Function name from list_function_intents"),
      detail: z
        .enum(["signature", "full"])
        .default("full")
        .describe("'signature' for metadata only, 'full' adds GitHub source coordinates"),
    },
    async (params) => getFunctionSource(client, params),
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
