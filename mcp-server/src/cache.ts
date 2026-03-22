import { CloudRepoData } from "./types.js";

const DEFAULT_TTL_MS = 5 * 60 * 1000; // 5 minutes

interface CacheEntry {
  data: CloudRepoData[];
  expiresAt: number;
}

/**
 * In-memory cache with TTL and Promise deduplication.
 * Ensures concurrent callers share a single in-flight fetch.
 */
export class Cache {
  private entry: CacheEntry | null = null;
  private inflight: Promise<CloudRepoData[]> | null = null;
  private ttlMs: number;

  constructor(ttlMs = DEFAULT_TTL_MS) {
    this.ttlMs = ttlMs;
  }

  async get(fetcher: () => Promise<CloudRepoData[]>): Promise<CloudRepoData[]> {
    // Return cached data if still valid
    if (this.entry && Date.now() < this.entry.expiresAt) {
      return this.entry.data;
    }

    // Deduplicate concurrent fetches
    if (this.inflight) {
      return this.inflight;
    }

    this.inflight = fetcher()
      .then((data) => {
        this.entry = {
          data,
          expiresAt: Date.now() + this.ttlMs,
        };
        return data;
      })
      .finally(() => {
        this.inflight = null;
      });

    return this.inflight;
  }

  invalidate(): void {
    this.entry = null;
  }
}
