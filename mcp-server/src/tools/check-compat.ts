import { ApiClient, matchService } from "../api-client.js";
import { normalizePath, pathsMatch } from "../utils/path-matcher.js";
import { CloudRepoData } from "../types.js";

export interface CheckCompatParams {
  consumer_service: string;
  producer_service: string;
  method?: string;
  path?: string;
}

interface CompatIssue {
  severity: "error" | "warning" | "info";
  category: string;
  message: string;
}

export async function checkCompatibility(
  client: ApiClient,
  params: CheckCompatParams,
) {
  const repos = await client.getAllRepoData();
  const consumer = matchService(repos, params.consumer_service);
  const producer = matchService(repos, params.producer_service);

  if (!consumer) {
    return errorResult(`Consumer service "${params.consumer_service}" not found.`);
  }
  if (!producer) {
    return errorResult(`Producer service "${params.producer_service}" not found.`);
  }

  const issues: CompatIssue[] = [];

  // Get consumer calls and producer endpoints
  const consumerCalls = getConsumerCalls(consumer);
  const producerEndpoints = getProducerEndpoints(producer);

  // Optionally filter by method/path
  let filteredCalls = consumerCalls;
  if (params.method) {
    const m = params.method.toUpperCase();
    filteredCalls = filteredCalls.filter((c) => c.method.toUpperCase() === m);
  }
  if (params.path) {
    filteredCalls = filteredCalls.filter((c) => pathsMatch(c.path, params.path!));
  }

  // Check each consumer call against producer endpoints
  for (const call of filteredCalls) {
    const matchingEndpoint = producerEndpoints.find(
      (e) =>
        e.method.toUpperCase() === call.method.toUpperCase() &&
        pathsMatch(e.path, call.path),
    );

    if (!matchingEndpoint) {
      issues.push({
        severity: "error",
        category: "missing_endpoint",
        message: `Consumer calls ${call.method.toUpperCase()} ${call.path} but producer has no matching endpoint`,
      });
    }
  }

  // Check for endpoints the consumer doesn't use (informational)
  for (const endpoint of producerEndpoints) {
    const hasConsumer = consumerCalls.some(
      (c) =>
        c.method.toUpperCase() === endpoint.method.toUpperCase() &&
        pathsMatch(c.path, endpoint.path),
    );

    if (!hasConsumer && !params.method && !params.path) {
      issues.push({
        severity: "info",
        category: "unused_endpoint",
        message: `Producer exposes ${endpoint.method.toUpperCase()} ${endpoint.path} but consumer doesn't call it`,
      });
    }
  }

  const compatible = !issues.some((i) => i.severity === "error");

  return {
    content: [
      {
        type: "text" as const,
        text: JSON.stringify(
          {
            consumer: consumer.service_name ?? consumer.repo_name,
            producer: producer.service_name ?? producer.repo_name,
            compatible,
            consumer_calls: filteredCalls.length,
            producer_endpoints: producerEndpoints.length,
            issues,
          },
          null,
          2,
        ),
      },
    ],
  };
}

function getConsumerCalls(repo: CloudRepoData) {
  if (repo.mount_graph?.data_calls?.length) {
    return repo.mount_graph.data_calls.map((c) => ({
      method: c.method,
      path: c.target_url,
    }));
  }
  return repo.calls.map((c) => ({
    method: c.method,
    path: c.route,
  }));
}

function getProducerEndpoints(repo: CloudRepoData) {
  if (repo.mount_graph?.endpoints?.length) {
    return repo.mount_graph.endpoints.map((e) => ({
      method: e.method,
      path: e.full_path,
    }));
  }
  return repo.endpoints.map((e) => ({
    method: e.method,
    path: e.route,
  }));
}

function errorResult(message: string) {
  return {
    content: [{ type: "text" as const, text: message }],
  };
}
