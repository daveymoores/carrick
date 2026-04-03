import { ApiClient } from "../api-client.js";
import { extractTypeDefinition } from "../utils/type-extractor.js";

export interface GetTypeDefinitionParams {
  service: string;
  type_alias: string;
}

export async function getTypeDefinition(
  client: ApiClient,
  params: GetTypeDefinitionParams,
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

  // First, try to find a manifest entry with a pre-resolved definition
  const entry = repo.type_manifest?.find(
    (e) => e.type_alias === params.type_alias,
  );

  if (entry?.resolved_definition) {
    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify(
            {
              service: repo.service_name ?? repo.repo_name,
              type_alias: params.type_alias,
              definition: entry.resolved_definition,
              ...(entry.expanded_definition
                ? { expanded: entry.expanded_definition }
                : {}),
            },
            null,
            2,
          ),
        },
      ],
    };
  }

  // Fallback: regex extract from bundled_types
  if (repo.bundled_types) {
    const definition = extractTypeDefinition(
      repo.bundled_types,
      params.type_alias,
    );
    if (definition) {
      return {
        content: [
          {
            type: "text" as const,
            text: JSON.stringify(
              {
                service: repo.service_name ?? repo.repo_name,
                type_alias: params.type_alias,
                definition,
                note: "Extracted via pattern matching — transitive dependencies may be missing.",
              },
              null,
              2,
            ),
          },
        ],
      };
    }
  }

  return {
    content: [
      {
        type: "text" as const,
        text: `Type "${params.type_alias}" not found in "${params.service}". Use get_endpoint_types to see available types for an endpoint.`,
      },
    ],
  };
}
