import { ApiClient } from "../api-client.js";

export async function getServiceTypes(
  client: ApiClient,
  serviceName: string,
): Promise<string> {
  const repo = await client.findService(serviceName);

  if (!repo) {
    return `// Service "${serviceName}" not found`;
  }

  if (!repo.bundled_types) {
    return `// No type definitions available for "${serviceName}"`;
  }

  return repo.bundled_types;
}
