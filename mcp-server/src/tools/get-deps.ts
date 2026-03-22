import { ApiClient } from "../api-client.js";

export interface GetDepsParams {
  service?: string;
}

interface DependencyConflict {
  package_name: string;
  severity: "error" | "warning";
  versions: Array<{
    service: string;
    version: string;
  }>;
}

export async function getServiceDependencies(
  client: ApiClient,
  params: GetDepsParams,
) {
  const repos = await client.getAllRepoData();

  if (params.service) {
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

    return {
      content: [
        {
          type: "text" as const,
          text: JSON.stringify(
            {
              service: repo.service_name ?? repo.repo_name,
              dependencies: repo.packages?.merged_dependencies ?? {},
              package_count: Object.keys(
                repo.packages?.merged_dependencies ?? {},
              ).length,
            },
            null,
            2,
          ),
        },
      ],
    };
  }

  // Org-wide: collect all dependencies and find conflicts
  const depMap = new Map<
    string,
    Array<{ service: string; version: string }>
  >();

  for (const repo of repos) {
    const serviceName = repo.service_name ?? repo.repo_name;
    const deps = repo.packages?.merged_dependencies ?? {};

    for (const [name, info] of Object.entries(deps)) {
      if (!depMap.has(name)) {
        depMap.set(name, []);
      }
      depMap.get(name)!.push({
        service: serviceName,
        version: info.version,
      });
    }
  }

  const conflicts: DependencyConflict[] = [];
  for (const [name, versions] of depMap) {
    const uniqueVersions = new Set(versions.map((v) => v.version));
    if (uniqueVersions.size > 1) {
      // Major version difference = error, minor = warning
      const majors = new Set(
        versions.map((v) => v.version.split(".")[0]),
      );
      conflicts.push({
        package_name: name,
        severity: majors.size > 1 ? "error" : "warning",
        versions,
      });
    }
  }

  // Sort: errors first, then by package name
  conflicts.sort((a, b) => {
    if (a.severity !== b.severity) {
      return a.severity === "error" ? -1 : 1;
    }
    return a.package_name.localeCompare(b.package_name);
  });

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(
          {
            total_packages: depMap.size,
            conflict_count: conflicts.length,
            conflicts,
          },
          null,
          2,
        ),
      },
    ],
  };
}
