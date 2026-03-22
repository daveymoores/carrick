import { CloudRepoData, CrossRepoResponse } from "./types.js";
import { Cache } from "./cache.js";

export interface ApiClientConfig {
  apiEndpoint: string;
  apiKey: string;
  org: string;
}

/**
 * HTTP client for the Carrick Lambda API.
 * Mirrors the Rust AwsStorage pattern: POST with Bearer auth.
 */
export class ApiClient {
  private lambdaUrl: string;
  private apiKey: string;
  private org: string;
  private cache: Cache;

  constructor(config: ApiClientConfig) {
    this.lambdaUrl = `${config.apiEndpoint}/types/check-or-upload`;
    this.apiKey = config.apiKey;
    this.org = config.org;
    this.cache = new Cache();
  }

  /**
   * Fetch all repo data for the configured org.
   * Uses in-memory cache with 5-min TTL and Promise deduplication.
   */
  async getAllRepoData(): Promise<CloudRepoData[]> {
    return this.cache.get(() => this.fetchCrossRepoData());
  }

  /**
   * Find repos matching a service name (fuzzy match).
   */
  async findService(name: string): Promise<CloudRepoData | undefined> {
    const repos = await this.getAllRepoData();
    return matchService(repos, name);
  }

  invalidateCache(): void {
    this.cache.invalidate();
  }

  private async fetchCrossRepoData(): Promise<CloudRepoData[]> {
    const response = await fetch(this.lambdaUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${this.apiKey}`,
      },
      body: JSON.stringify({
        action: "get-cross-repo-data",
        org: this.org,
      }),
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`Lambda returned ${response.status}: ${text}`);
    }

    const data: CrossRepoResponse = await response.json();
    return data.repos
      .filter((r) => r.metadata != null)
      .map((r) => r.metadata!);
  }
}

/**
 * Fuzzy service matching: match by repo_name, service_name, or trailing segment.
 * e.g. "order-service" matches "daveymoores/order-service"
 */
export function matchService(
  repos: CloudRepoData[],
  name: string,
): CloudRepoData | undefined {
  const lower = name.toLowerCase();

  // Exact match on service_name
  const byService = repos.find(
    (r) => r.service_name?.toLowerCase() === lower,
  );
  if (byService) return byService;

  // Exact match on repo_name
  const byRepo = repos.find((r) => r.repo_name.toLowerCase() === lower);
  if (byRepo) return byRepo;

  // Trailing segment match (e.g. "order-service" matches "org/order-service")
  const bySegment = repos.find((r) => {
    const segments = r.repo_name.toLowerCase().split("/");
    return segments[segments.length - 1] === lower;
  });
  if (bySegment) return bySegment;

  // Partial match on service_name or repo_name
  return repos.find(
    (r) =>
      r.service_name?.toLowerCase().includes(lower) ||
      r.repo_name.toLowerCase().includes(lower),
  );
}
