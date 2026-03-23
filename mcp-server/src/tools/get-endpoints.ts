import { ApiClient } from "../api-client.js";
import { pathContains } from "../utils/path-matcher.js";

export interface GetEndpointsParams {
  service: string;
  method?: string;
  path_contains?: string;
}

export async function getEndpoints(
  client: ApiClient,
  params: GetEndpointsParams,
) {
  const repo = await client.findService(params.service);
  if (!repo) {
    return {
      content: [
        {
          type: "text" as const,
          text: `Service "${params.service}" not found. Use list_services to see available services.`,
        },
      ],
    };
  }

  // Prefer mount_graph.endpoints (resolved full_path) over top-level endpoints
  let endpoints: Array<{
    method: string;
    full_path: string;
    handler?: string;
    owner?: string;
    file_location: string;
  }>;

  if (repo.mount_graph?.endpoints?.length) {
    endpoints = repo.mount_graph.endpoints.map((e) => ({
      method: e.method,
      full_path: e.full_path,
      handler: e.handler ?? undefined,
      owner: e.owner,
      file_location: e.file_location,
    }));
  } else {
    endpoints = repo.endpoints.map((e) => ({
      method: e.method,
      full_path: e.route,
      handler: e.handler_name ?? undefined,
      owner: e.owner ? ownerName(e.owner) : undefined,
      file_location: e.file_path,
    }));
  }

  // Apply filters
  if (params.method) {
    const m = params.method.toUpperCase();
    endpoints = endpoints.filter((e) => e.method.toUpperCase() === m);
  }

  if (params.path_contains) {
    endpoints = endpoints.filter((e) =>
      pathContains(e.full_path, params.path_contains!),
    );
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(
          {
            service: repo.service_name ?? repo.repo_name,
            endpoint_count: endpoints.length,
            endpoints,
          },
          null,
          2,
        ),
      },
    ],
  };
}

function ownerName(owner: { App?: string; Router?: string; Middleware?: string }): string {
  return (owner as Record<string, string>).App
    ?? (owner as Record<string, string>).Router
    ?? (owner as Record<string, string>).Middleware
    ?? "unknown";
}
