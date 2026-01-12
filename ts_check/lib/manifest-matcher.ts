/**
 * Manifest-Based Type Matcher
 *
 * This module provides matching between producer and consumer type manifests
 * based on HTTP method and path. It handles various path parameter formats
 * and normalizes paths for accurate matching.
 */

import * as fs from "fs";
import * as path from "path";

// ============================================================================
// Type Definitions
// ============================================================================

/**
 * The type manifest containing all discovered types for a repository
 */
export interface TypeManifest {
  /** Name of the repository */
  repo_name: string;
  /** Git commit hash at time of analysis */
  commit_hash: string;
  /** All manifest entries */
  entries: ManifestEntry[];
}

/**
 * A single entry in the type manifest
 */
export type ManifestRole = 'producer' | 'consumer';
export type ManifestTypeKind = 'request' | 'response';
export type ManifestTypeState = 'explicit' | 'implicit' | 'unknown';

export interface ManifestEntry {
  /** HTTP method (GET, POST, PUT, DELETE, etc.) */
  method: string;
  /** API path (e.g., /api/users/:id) */
  path: string;
  /** The type alias for this endpoint */
  type_alias: string;
  /** Whether this is a producer or consumer */
  role: ManifestRole;
  /** Whether this entry represents request or response */
  type_kind: ManifestTypeKind;
  /** Source file path where the type was found */
  file_path: string;
  /** Line number in the source file */
  line_number: number;
  /** Whether the type was explicitly annotated */
  is_explicit: boolean;
  /** Current state of the type extraction */
  type_state: ManifestTypeState;
}

/**
 * Result of matching a producer-consumer pair
 */
export interface MatchResult {
  /** The HTTP method */
  method: string;
  /** The normalized API path */
  path: string;
  /** The type kind being matched */
  type_kind: ManifestTypeKind;
  /** The producer manifest entry */
  producer: ManifestEntry;
  /** The consumer manifest entry */
  consumer: ManifestEntry;
  /** Score indicating match quality (1.0 = exact match) */
  match_score: number;
}

/**
 * Result of finding orphaned entries (unmatched producers or consumers)
 */
export interface OrphanedEntry {
  /** The unmatched entry */
  entry: ManifestEntry;
  /** Reason why no match was found */
  reason: string;
}

// ============================================================================
// Path Normalization
// ============================================================================

/**
 * Normalize a path for comparison
 *
 * This handles:
 * - Trailing slashes
 * - Path parameter formats (:id, {id}, [id])
 * - Case normalization
 * - Multiple slashes
 */
export function normalizePath(inputPath: string): string {
  let normalized = inputPath;

  // Convert to lowercase for case-insensitive comparison
  normalized = normalized.toLowerCase();

  // Remove trailing slashes (but keep leading slash)
  normalized = normalized.replace(/\/+$/, '');

  // Ensure leading slash
  if (!normalized.startsWith('/')) {
    normalized = '/' + normalized;
  }

  // Normalize multiple slashes to single slash
  normalized = normalized.replace(/\/+/g, '/');

  // Convert all path parameter formats to canonical form :param
  // Handle Express style :id
  // Handle OpenAPI style {id}
  // Handle Next.js style [id]
  normalized = normalized.replace(/\{([^}]+)\}/g, ':$1');
  normalized = normalized.replace(/\[([^\]]+)\]/g, ':$1');

  // Normalize path parameters to a generic placeholder for matching
  // This allows :id, :userId, :user_id to all match
  normalized = normalized.replace(/:[\w-]+/g, ':param');

  return normalized;
}

/**
 * Normalize HTTP method to uppercase
 */
export function normalizeMethod(method: string): string {
  return method.toUpperCase();
}

// ============================================================================
// ManifestMatcher Class
// ============================================================================

/**
 * ManifestMatcher - Matches producer and consumer type manifests
 *
 * Usage:
 *   const matcher = new ManifestMatcher();
 *   const producers = matcher.loadManifest('./producer-manifest.json');
 *   const consumers = matcher.loadManifest('./consumer-manifest.json');
 *   const matches = matcher.matchEndpoints(producers, consumers);
 */
export class ManifestMatcher {
  /**
   * Load a manifest from a JSON file
   *
   * @param jsonPath - Path to the manifest JSON file
   * @returns The parsed TypeManifest
   * @throws Error if file doesn't exist or is invalid JSON
   */
  loadManifest(jsonPath: string): TypeManifest {
    const absolutePath = path.isAbsolute(jsonPath)
      ? jsonPath
      : path.resolve(process.cwd(), jsonPath);

    if (!fs.existsSync(absolutePath)) {
      throw new Error(`Manifest file not found: ${absolutePath}`);
    }

    const content = fs.readFileSync(absolutePath, 'utf-8');

    try {
      const manifest = JSON.parse(content) as TypeManifest;

      // Validate required fields
      if (!manifest.repo_name) {
        throw new Error('Manifest missing required field: repo_name');
      }
      if (!manifest.commit_hash) {
        throw new Error('Manifest missing required field: commit_hash');
      }
      if (!Array.isArray(manifest.entries)) {
        throw new Error('Manifest missing required field: entries (must be an array)');
      }

      // Validate entries
      for (const entry of manifest.entries) {
        this.validateEntry(entry);
      }

      return manifest;
    } catch (err) {
      if (err instanceof SyntaxError) {
        throw new Error(`Invalid JSON in manifest file: ${absolutePath}`);
      }
      throw err;
    }
  }

  /**
   * Parse manifest from a JSON string
   *
   * @param jsonContent - The JSON string to parse
   * @returns The parsed TypeManifest
   */
  parseManifest(jsonContent: string): TypeManifest {
    const manifest = JSON.parse(jsonContent) as TypeManifest;

    // Validate required fields
    if (!manifest.repo_name) {
      throw new Error('Manifest missing required field: repo_name');
    }
    if (!manifest.commit_hash) {
      throw new Error('Manifest missing required field: commit_hash');
    }
    if (!Array.isArray(manifest.entries)) {
      throw new Error('Manifest missing required field: entries (must be an array)');
    }

    return manifest;
  }

  /**
   * Validate a manifest entry has all required fields
   */
  private validateEntry(entry: ManifestEntry): void {
    if (!entry.method) {
      throw new Error('ManifestEntry missing required field: method');
    }
    if (!entry.path) {
      throw new Error('ManifestEntry missing required field: path');
    }
    if (!entry.type_alias) {
      throw new Error('ManifestEntry missing required field: type_alias');
    }
    if (!entry.role || !['producer', 'consumer'].includes(entry.role)) {
      throw new Error('ManifestEntry missing or invalid field: role (must be "producer" or "consumer")');
    }
    if (!entry.type_kind || !['request', 'response'].includes(entry.type_kind)) {
      throw new Error('ManifestEntry missing or invalid field: type_kind (must be "request" or "response")');
    }
    if (!entry.file_path) {
      throw new Error('ManifestEntry missing required field: file_path');
    }
    if (typeof entry.line_number !== 'number') {
      throw new Error('ManifestEntry missing required field: line_number (must be a number)');
    }
    if (typeof entry.is_explicit !== 'boolean') {
      throw new Error('ManifestEntry missing required field: is_explicit (must be a boolean)');
    }
    if (!entry.type_state || !['explicit', 'implicit', 'unknown'].includes(entry.type_state)) {
      throw new Error('ManifestEntry missing or invalid field: type_state (must be "explicit", "implicit", or "unknown")');
    }
  }

  /**
   * Find all producer entries for a specific endpoint
   *
   * @param manifest - The manifest to search
   * @param method - HTTP method to match
   * @param path - API path to match
   * @returns Array of matching producer entries
   */
  findProducersForEndpoint(
    manifest: TypeManifest,
    method: string,
    inputPath: string,
    typeKind?: ManifestTypeKind
  ): ManifestEntry[] {
    const normalizedMethod = normalizeMethod(method);
    const normalizedPath = normalizePath(inputPath);

    return manifest.entries.filter((entry) => {
      if (entry.role !== 'producer') return false;
      if (typeKind && entry.type_kind !== typeKind) return false;

      const entryMethod = normalizeMethod(entry.method);
      const entryPath = normalizePath(entry.path);

      return entryMethod === normalizedMethod && entryPath === normalizedPath;
    });
  }

  /**
   * Find all consumer entries for a specific endpoint
   *
   * @param manifest - The manifest to search
   * @param method - HTTP method to match
   * @param path - API path to match
   * @returns Array of matching consumer entries
   */
  findConsumersForEndpoint(
    manifest: TypeManifest,
    method: string,
    inputPath: string,
    typeKind?: ManifestTypeKind
  ): ManifestEntry[] {
    const normalizedMethod = normalizeMethod(method);
    const normalizedPath = normalizePath(inputPath);

    return manifest.entries.filter((entry) => {
      if (entry.role !== 'consumer') return false;
      if (typeKind && entry.type_kind !== typeKind) return false;

      const entryMethod = normalizeMethod(entry.method);
      const entryPath = normalizePath(entry.path);

      return entryMethod === normalizedMethod && entryPath === normalizedPath;
    });
  }

  /**
   * Get all unique endpoints from a manifest
   *
   * @param manifest - The manifest to process
   * @returns Array of unique {method, path} pairs
   */
  getUniqueEndpoints(
    manifest: TypeManifest
  ): Array<{ method: string; path: string; type_kind: ManifestTypeKind }> {
    const seen = new Set<string>();
    const endpoints: Array<{ method: string; path: string; type_kind: ManifestTypeKind }> = [];

    for (const entry of manifest.entries) {
      const normalizedMethod = normalizeMethod(entry.method);
      const normalizedPath = normalizePath(entry.path);
      const key = `${normalizedMethod} ${normalizedPath} ${entry.type_kind}`;

      if (!seen.has(key)) {
        seen.add(key);
        endpoints.push({
          method: normalizedMethod,
          path: normalizedPath,
          type_kind: entry.type_kind,
        });
      }
    }

    return endpoints;
  }

  /**
   * Match producer and consumer manifests to find endpoint pairs
   *
   * @param producers - Manifest containing producer types
   * @param consumers - Manifest containing consumer types
   * @returns Object containing matched pairs and orphaned entries
   */
  matchEndpoints(
    producers: TypeManifest,
    consumers: TypeManifest
  ): {
    matches: MatchResult[];
    orphanedProducers: OrphanedEntry[];
    orphanedConsumers: OrphanedEntry[];
  } {
    const matches: MatchResult[] = [];
    const orphanedProducers: OrphanedEntry[] = [];
    const orphanedConsumers: OrphanedEntry[] = [];

    // Track which entries have been matched
    const matchedProducerIndices = new Set<number>();
    const matchedConsumerIndices = new Set<number>();

    // Get producer entries
    const producerEntries = producers.entries.filter((e) => e.role === 'producer');
    const consumerEntries = consumers.entries.filter((e) => e.role === 'consumer');

    // For each consumer, try to find matching producers
    for (let ci = 0; ci < consumerEntries.length; ci++) {
      const consumer = consumerEntries[ci];
      const consumerMethod = normalizeMethod(consumer.method);
      const consumerPath = normalizePath(consumer.path);

      let foundMatch = false;

      for (let pi = 0; pi < producerEntries.length; pi++) {
        const producer = producerEntries[pi];
        const producerMethod = normalizeMethod(producer.method);
        const producerPath = normalizePath(producer.path);

        if (
          consumerMethod === producerMethod &&
          consumerPath === producerPath &&
          consumer.type_kind === producer.type_kind
        ) {
          matches.push({
            method: consumerMethod,
            path: consumerPath,
            type_kind: consumer.type_kind,
            producer,
            consumer,
            match_score: this.calculateMatchScore(producer, consumer),
          });

          matchedProducerIndices.add(pi);
          matchedConsumerIndices.add(ci);
          foundMatch = true;
          // Don't break - a consumer might match multiple producers (e.g., different versions)
        }
      }

      if (!foundMatch) {
        orphanedConsumers.push({
          entry: consumer,
          reason: `No producer found for ${consumer.method} ${consumer.path} (${consumer.type_kind})`,
        });
      }
    }

    // Find orphaned producers (producers with no matching consumers)
    for (let pi = 0; pi < producerEntries.length; pi++) {
      if (!matchedProducerIndices.has(pi)) {
        const producer = producerEntries[pi];
        orphanedProducers.push({
          entry: producer,
          reason: `No consumer found for ${producer.method} ${producer.path} (${producer.type_kind})`,
        });
      }
    }

    return {
      matches,
      orphanedProducers,
      orphanedConsumers,
    };
  }

  /**
   * Calculate a match score between producer and consumer
   *
   * Currently returns 1.0 for all matches, but could be extended
   * to consider factors like:
   * - Exact path parameter names vs normalized
   * - Similar paths that almost match
   * - Version compatibility
   */
  private calculateMatchScore(producer: ManifestEntry, consumer: ManifestEntry): number {
    const producerMethod = normalizeMethod(producer.method);
    const consumerMethod = normalizeMethod(consumer.method);
    const producerPath = normalizePath(producer.path);
    const consumerPath = normalizePath(consumer.path);

    // Exact normalized match
    if (producerMethod === consumerMethod && producerPath === consumerPath) {
      // Check if original paths are identical (higher confidence)
      if (
        producer.method.toUpperCase() === consumer.method.toUpperCase() &&
        producer.path === consumer.path
      ) {
        return 1.0;
      }
      // Normalized match (slightly lower confidence due to normalization)
      return 0.95;
    }

    return 0;
  }

  /**
   * Create an empty manifest
   *
   * @param repoName - Name of the repository
   * @param commitHash - Git commit hash
   * @returns An empty TypeManifest
   */
  createEmptyManifest(repoName: string, commitHash: string): TypeManifest {
    return {
      repo_name: repoName,
      commit_hash: commitHash,
      entries: [],
    };
  }

  /**
   * Add an entry to a manifest
   *
   * @param manifest - The manifest to modify
   * @param entry - The entry to add
   */
  addEntry(manifest: TypeManifest, entry: ManifestEntry): void {
    this.validateEntry(entry);
    manifest.entries.push(entry);
  }

  /**
   * Serialize a manifest to JSON
   *
   * @param manifest - The manifest to serialize
   * @param pretty - Whether to pretty-print the JSON
   * @returns JSON string representation
   */
  serializeManifest(manifest: TypeManifest, pretty: boolean = true): string {
    return JSON.stringify(manifest, null, pretty ? 2 : 0);
  }

  /**
   * Save a manifest to a file
   *
   * @param manifest - The manifest to save
   * @param filePath - Path to save the manifest
   */
  saveManifest(manifest: TypeManifest, filePath: string): void {
    const absolutePath = path.isAbsolute(filePath)
      ? filePath
      : path.resolve(process.cwd(), filePath);

    const content = this.serializeManifest(manifest);
    fs.writeFileSync(absolutePath, content, 'utf-8');
  }
}

// ============================================================================
// Utility Functions
// ============================================================================

/**
 * Create a manifest entry helper
 */
export function createManifestEntry(
  method: string,
  entryPath: string,
  typeAlias: string,
  role: ManifestRole,
  filePath: string,
  lineNumber: number,
  typeKind: ManifestTypeKind = 'response',
  isExplicit: boolean = true,
  typeState: ManifestTypeState = isExplicit ? 'explicit' : 'implicit'
): ManifestEntry {
  return {
    method: normalizeMethod(method),
    path: entryPath,
    type_alias: typeAlias,
    role,
    type_kind: typeKind,
    file_path: filePath,
    line_number: lineNumber,
    is_explicit: isExplicit,
    type_state: typeState,
  };
}

/**
 * Merge multiple manifests from the same role into one
 *
 * @param manifests - Array of manifests to merge
 * @param repoName - Name for the merged manifest
 * @param commitHash - Commit hash for the merged manifest
 * @returns Merged TypeManifest
 */
export function mergeManifests(
  manifests: TypeManifest[],
  repoName: string,
  commitHash: string
): TypeManifest {
  const merged: TypeManifest = {
    repo_name: repoName,
    commit_hash: commitHash,
    entries: [],
  };

  for (const manifest of manifests) {
    merged.entries.push(...manifest.entries);
  }

  return merged;
}

// Export default matcher instance for convenience
export const defaultMatcher = new ManifestMatcher();
