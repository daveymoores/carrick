import { ApiClient } from "../api-client.js";

export async function getServiceCatalog(client: ApiClient): Promise<string> {
  const repos = await client.getAllRepoData();

  const catalog = repos.map((repo) => {
    const endpoints =
      repo.mount_graph?.endpoints ?? repo.endpoints ?? [];
    const calls =
      repo.mount_graph?.data_calls ?? repo.calls ?? [];

    return {
      repo_name: repo.repo_name,
      service_name: repo.service_name ?? repo.repo_name,
      endpoint_count: endpoints.length,
      call_count: calls.length,
      last_updated: repo.last_updated,
      commit_hash: repo.commit_hash,
      has_types: repo.bundled_types != null && repo.bundled_types.length > 0,
    };
  });

  return JSON.stringify(catalog, null, 2);
}
