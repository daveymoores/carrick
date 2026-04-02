import { ApiClient } from "../api-client.js";
import { extractEndpointTypes } from "../utils/type-extractor.js";
import { pathsMatch } from "../utils/path-matcher.js";

export interface GetTypesParams {
  service: string;
  method: string;
  path: string;
}

export async function getEndpointTypes(
  client: ApiClient,
  params: GetTypesParams,
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

  if (!repo.type_manifest || !repo.bundled_types) {
    return {
      content: [
        {
          type: "text" as const,
          text: `Service "${params.service}" has no type information available. Run Carrick analysis with type extraction enabled.`,
        },
      ],
    };
  }

  // Verify the endpoint actually exists
  const endpoints = repo.mount_graph?.endpoints ?? [];
  const topLevelEndpoints = repo.endpoints ?? [];
  const endpointExists =
    endpoints.some(
      (e) =>
        e.method.toUpperCase() === params.method.toUpperCase() &&
        pathsMatch(e.full_path, params.path),
    ) ||
    topLevelEndpoints.some(
      (e) =>
        e.method.toUpperCase() === params.method.toUpperCase() &&
        pathsMatch(e.route, params.path),
    );

  const types = extractEndpointTypes(
    repo.type_manifest,
    repo.bundled_types,
    params.method,
    params.path,
  );

  if (types.length === 0) {
    return {
      content: [
        {
          type: "text" as const,
          text: endpointExists
            ? `Endpoint ${params.method.toUpperCase()} ${params.path} exists but has no extracted types. The types may not have been annotated or extracted.`
            : `No endpoint matching ${params.method.toUpperCase()} ${params.path} found in "${params.service}". Use get_api_endpoints to see available endpoints.`,
        },
      ],
    };
  }

  const result = {
    service: repo.service_name ?? repo.repo_name,
    method: params.method.toUpperCase(),
    path: params.path,
    types: types.map((t) => ({
      type_alias: t.type_alias,
      type_kind: t.type_kind,
      is_explicit: t.is_explicit,
      source_file: t.file_path,
      source_line: t.line_number,
      definition: t.definition,
      ...(t.expanded ? { expanded: t.expanded } : {}),
    })),
    hint: "Use get_type_definition(service, type_alias) to get the full resolved definition of any type, including all transitive dependencies.",
  };

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(result, null, 2),
      },
    ],
  };
}
