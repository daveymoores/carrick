import { ApiClient } from "../api-client.js";
import type { FunctionDefinition, FunctionCallRef } from "../types.js";

export interface ListFunctionIntentsParams {
  service?: string;
  exclude_service?: string;
}

interface FunctionIntentEntry {
  service: string;
  repo: string;
  name: string;
  file_path: string;
  line_number: number;
  intent: string;
  calls?: FunctionCallRef[];
}

/**
 * List all exported functions with their intents across the org.
 * Returns a compact list that an LLM agent can scan to find relevant functions.
 * The agent can then use `gh` CLI to browse the actual source code on GitHub.
 */
export async function listFunctionIntents(
  client: ApiClient,
  params: ListFunctionIntentsParams,
) {
  const repos = await client.getAllRepoData();
  const entries: FunctionIntentEntry[] = [];

  for (const repo of repos) {
    const serviceName = repo.service_name ?? repo.repo_name;

    if (params.service && serviceName !== params.service) continue;
    if (params.exclude_service && serviceName === params.exclude_service) continue;

    for (const [, def] of Object.entries(repo.function_definitions)) {
      const fn = def as FunctionDefinition;
      if (!fn.is_exported || !fn.intent) continue;

      entries.push({
        service: serviceName,
        repo: repo.repo_name,
        name: fn.name,
        file_path: fn.file_path,
        line_number: fn.line_number,
        intent: fn.intent,
        ...(fn.calls && fn.calls.length > 0 ? { calls: fn.calls } : {}),
      });
    }
  }

  if (entries.length === 0) {
    return {
      content: [
        {
          type: "text" as const,
          text: params.service
            ? `No exported functions with intents found for "${params.service}". Run analysis with the latest Carrick version to generate function intents.`
            : `No exported functions with intents found across ${repos.length} services. Run analysis with the latest Carrick version to generate function intents.`,
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
            total: entries.length,
            services: [...new Set(entries.map((e) => e.service))],
            functions: entries,
            hint: "To view a function's source code, use the gh CLI: gh api repos/{owner}/{repo}/contents/{file_path} or gh browse {owner}/{repo} -- {file_path}:{line_number}",
          },
          null,
          2,
        ),
      },
    ],
  };
}
