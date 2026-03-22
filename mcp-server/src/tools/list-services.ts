import { ApiClient } from "../api-client.js";

export async function listServices(client: ApiClient) {
  const repos = await client.getAllRepoData();

  const services = repos.map((repo) => {
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

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(services, null, 2),
      },
    ],
  };
}
