import { ApiClient } from "../api-client.js";
import { extractEndpointTypes } from "../utils/type-extractor.js";
import { extractPathParams, pathsMatch } from "../utils/path-matcher.js";
import type {
  CloudRepoData,
  FunctionDefinition,
  ResolvedEndpoint,
} from "../types.js";

export interface GetFunctionSourceParams {
  service: string;
  function_name: string;
  detail?: "signature" | "full";
}

interface ResolvedType {
  alias: string;
  /** Compiler-expanded definition string, or null if unresolvable. */
  definition: string | null;
  is_explicit?: boolean;
  reason?: string;
}

interface ExternalCall {
  service: string;
  method: string;
  path: string;
}

interface SourceCoordinates {
  repo: string;
  path: string;
  ref: string;
  start_line: number;
  end_line: number;
}

interface GetFunctionSourceResult {
  function_name: string;
  service: string;
  route: string | null;
  params: Record<string, string> | null;
  request_body: ResolvedType | null;
  response_type: ResolvedType | null;
  external_calls: ExternalCall[];
  internal_calls: string[];
  intent: string | null;
  source?: SourceCoordinates;
}

export async function getFunctionSource(
  client: ApiClient,
  params: GetFunctionSourceParams,
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

  const fn = repo.function_definitions[params.function_name] as
    | FunctionDefinition
    | undefined;
  if (!fn) {
    return {
      content: [
        {
          type: "text" as const,
          text: `Function "${params.function_name}" not found in "${params.service}". Use list_function_intents to see available functions.`,
        },
      ],
    };
  }

  const endpoint = repo.mount_graph?.endpoints?.find(
    (e) => e.handler === fn.name,
  );

  const allRepos = await client.getAllRepoData();

  const result: GetFunctionSourceResult = {
    function_name: fn.name,
    service: repo.service_name ?? repo.repo_name,
    route: endpoint
      ? `${endpoint.method.toUpperCase()} ${endpoint.full_path}`
      : null,
    params: endpoint ? nullIfEmpty(extractPathParams(endpoint.full_path)) : null,
    request_body: resolveEndpointType(repo, endpoint, "request"),
    response_type: resolveEndpointType(repo, endpoint, "response"),
    external_calls: resolveExternalCalls(repo, fn, allRepos),
    internal_calls: (fn.calls ?? []).map((c) => c.name),
    intent: fn.intent ?? null,
  };

  if ((params.detail ?? "full") === "full") {
    const source = buildSourceCoordinates(repo, fn);
    if (source) result.source = source;
  }

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(result, null, 2),
      },
    ],
  };
}

function nullIfEmpty<T extends object>(obj: T): T | null {
  return Object.keys(obj).length === 0 ? null : obj;
}

/**
 * Resolve a request or response type for an endpoint via the existing
 * type-extractor utility, returning a structured `ResolvedType` for the agent.
 */
function resolveEndpointType(
  repo: CloudRepoData,
  endpoint: ResolvedEndpoint | undefined,
  kind: "request" | "response",
): ResolvedType | null {
  if (!endpoint || !repo.type_manifest) return null;

  const extracted = extractEndpointTypes(
    repo.type_manifest,
    repo.bundled_types ?? "",
    endpoint.method,
    endpoint.full_path,
  ).find((t) => t.type_kind === kind);

  if (!extracted) return null;

  // Prefer the compiler-expanded form (fully inlined) when available, since
  // the agent gets a self-contained type without needing follow-up lookups.
  const definition = extracted.expanded ?? extracted.definition ?? null;

  return {
    alias: extracted.type_alias,
    definition,
    is_explicit: extracted.is_explicit,
    ...(definition === null ? { reason: "unresolved" } : {}),
  };
}

/**
 * Resolve external API calls made from the function's file. Each call is
 * mapped to its target service by matching the call's URL path against
 * known endpoints across the org.
 *
 * Note: this returns calls scoped to the function's file, not the function
 * itself — `DataFetchingCall` doesn't currently carry line numbers, so we
 * can't filter to the function's body. Agents should use the source
 * coordinates to verify which calls actually belong to this function.
 */
function resolveExternalCalls(
  repo: CloudRepoData,
  fn: FunctionDefinition,
  allRepos: CloudRepoData[],
): ExternalCall[] {
  const dataCalls = repo.mount_graph?.data_calls ?? [];
  const funcFile = normalizeFilePath(fn.file_path);

  return dataCalls
    .filter((call) => normalizeFilePath(call.file_location) === funcFile)
    .map((call) => {
      const path = extractPathFromUrl(call.target_url);
      return {
        service: resolveCallService(call.method, path, allRepos) ?? path,
        method: call.method.toUpperCase(),
        path,
      };
    });
}

function normalizeFilePath(p: string): string {
  return p.replace(/^\.\//, "");
}

/**
 * Pull a request path out of a call URL. Strips scheme/host/env-var prefixes
 * and query strings so the result can be matched against a service's routes.
 */
function extractPathFromUrl(url: string): string {
  let cleaned = url
    .replace(/\$\{[^}]+\}/g, "")
    .replace(/\$[A-Z_][A-Z0-9_]*/g, "");

  try {
    if (/^https?:\/\//.test(cleaned)) {
      cleaned = new URL(cleaned).pathname;
    }
  } catch {
    // fall through with the cleaned string
  }

  // Drop query string and trailing whitespace, ensure leading slash
  cleaned = cleaned.split("?")[0].trim();
  if (!cleaned.startsWith("/")) cleaned = "/" + cleaned;
  return cleaned;
}

/**
 * Find which service in the org owns an endpoint matching the given
 * method+path. Returns the service name, or undefined if no match.
 */
function resolveCallService(
  method: string,
  path: string,
  allRepos: CloudRepoData[],
): string | undefined {
  const m = method.toUpperCase();
  for (const r of allRepos) {
    const endpoints = r.mount_graph?.endpoints ?? [];
    if (
      endpoints.some(
        (e) => e.method.toUpperCase() === m && pathsMatch(e.full_path, path),
      )
    ) {
      return r.service_name ?? r.repo_name;
    }
  }
  return undefined;
}

function buildSourceCoordinates(
  repo: CloudRepoData,
  fn: FunctionDefinition,
): SourceCoordinates | null {
  if (!fn.line_number || !fn.end_line_number) return null;
  return {
    repo: repo.repo_name,
    path: fn.file_path,
    ref: repo.commit_hash,
    start_line: fn.line_number,
    end_line: fn.end_line_number,
  };
}
