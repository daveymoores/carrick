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
export type InferKind = string;

export interface TypeEvidence {
  /** Source file path where the type was found */
  file_path: string;
  /** Start byte offset in the source file (nullable when unavailable) */
  span_start: number | null;
  /** End byte offset in the source file (nullable when unavailable) */
  span_end: number | null;
  /** Line number in the source file */
  line_number: number;
  /** Kind of inference performed for this type */
  infer_kind: InferKind;
  /** Whether the type was explicitly annotated */
  is_explicit: boolean;
  /** Current state of the type extraction */
  type_state: ManifestTypeState;
}

/** Socket message-flow direction, serialised snake_case by the Rust scanner. */
export type SocketDirection = 'server_to_client' | 'client_to_server';

export interface ManifestEntry {
  /**
   * Protocol tag carried by the operation key. ts_check runs the same
   * `TypeCompatibilityChecker` assignability for every protocol it understands:
   * "http" (matched by method/path), "socket" (matched by event+direction), and
   * "graphql" (matched by operation kind+field). Other protocols are checked by
   * their own pipelines and are dropped here.
   */
  protocol: 'http' | 'socket' | 'graphql';
  /** HTTP method (GET, POST, …). Present on HTTP entries; absent on socket/graphql. */
  method?: string;
  /** API path (e.g., /api/users/:id). Present on HTTP entries; absent on socket/graphql. */
  path?: string;
  /** Socket event name (e.g. `payment:settled`). Present on socket entries only. */
  event?: string;
  /** Socket message-flow direction. Present on socket entries only. */
  direction?: SocketDirection;
  /**
   * GraphQL root-operation kind (`query`/`mutation`/`subscription`). Present on
   * graphql entries only; serialised lowercase by the Rust scanner
   * (`GraphqlOperationKind::as_str`).
   */
  kind?: string;
  /**
   * GraphQL top-level field name (e.g. `order`). Present on graphql entries
   * only; the field that, with `kind`, identifies the operation.
   */
  field?: string;
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
  /** Evidence metadata for how this entry was derived */
  evidence: TypeEvidence;
}

/**
 * Result of matching a producer-consumer pair
 */
export interface MatchResult {
  /**
   * The pseudo-method this match is keyed on: the HTTP method for HTTP edges,
   * or the literal `SOCKET` for socket edges. Combined with `path` it forms the
   * `endpoint` label the Rust verdict-join parses back (`parse_compat_endpoint`).
   */
  method: string;
  /**
   * The identity this match is keyed on: the normalized API path for HTTP, or
   * the socket canonical `<DIRECTION>|<event>` tail for socket edges (so the
   * full label `SOCKET <DIRECTION>|<event>` parses back into the same join key
   * as the Rust `socket|<DIRECTION>|<event>` producer key).
   */
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

  // Convert ${expr} template literal remnants to :param
  normalized = normalized.replace(/\$\{[^}]+\}/g, ':param');

  // Normalize all path parameter names to :param for deduplication
  normalized = normalized.replace(/:[^/]+/g, ':param');

  return normalized;
}

/**
 * Check if two paths match using route-aware segment comparison.
 *
 * Treats any segment starting with ':' as a wildcard that matches any
 * other segment. This mirrors the semantics of routing libraries like
 * Express (path-to-regexp) and matchit.
 *
 * Examples:
 *   pathsMatch('/api/orders/:id', '/api/orders/101')       → true
 *   pathsMatch('/users/:id', '/users/:userId')              → true
 *   pathsMatch('/api/orders/:id', '/api/orders/abc-123')    → true
 *   pathsMatch('/api/users', '/api/orders')                 → false
 */
export function pathsMatch(path1: string, path2: string): boolean {
  const norm1 = normalizePath(path1);
  const norm2 = normalizePath(path2);

  // Fast path: exact normalized match
  if (norm1 === norm2) return true;

  const segs1 = norm1.split('/');
  const segs2 = norm2.split('/');

  if (segs1.length !== segs2.length) return false;

  return segs1.every(
    (seg, i) => seg === segs2[i] || seg.startsWith(':') || segs2[i].startsWith(':')
  );
}

/**
 * Normalize HTTP method to uppercase
 */
export function normalizeMethod(method: string): string {
  return method.toUpperCase();
}

// ============================================================================
// Socket Identity
// ============================================================================

/** The pseudo-method socket matches are keyed on, mirroring HTTP's method. */
export const SOCKET_PSEUDO_METHOD = 'SOCKET';

/**
 * ASCII direction label used in the canonical socket key, byte-identical to the
 * Rust `SocketDirection::label()` (`SERVER->CLIENT` / `CLIENT->SERVER`). The
 * Rust scanner serialises the direction snake_case (`server_to_client`); the
 * canonical key it builds with `OperationKey::canonical()` uses these labels, so
 * ts_check must reconstruct the same label to join a socket edge to its verdict.
 */
export function socketDirectionLabel(direction: SocketDirection): string {
  return direction === 'server_to_client' ? 'SERVER->CLIENT' : 'CLIENT->SERVER';
}

/**
 * Stable identity for a socket entry: `<DIRECTION>|<event>`. Two socket entries
 * match iff this string is equal (same event flowing the same direction), the
 * exact-key semantics the Rust `analyze_exact_key_matches` uses. The full
 * canonical key on the Rust side is `socket|<DIRECTION>|<event>`; this is its
 * `<DIRECTION>|<event>` tail, which becomes the `path` of the socket
 * `MatchResult` so the assembled `endpoint` label round-trips through the
 * verdict-join.
 */
export function socketKey(entry: ManifestEntry): string | null {
  if (entry.protocol !== 'socket' || !entry.event || !entry.direction) {
    return null;
  }
  return `${socketDirectionLabel(entry.direction)}|${entry.event}`;
}

// ============================================================================
// GraphQL Identity
// ============================================================================

/** The pseudo-method graphql matches are keyed on, mirroring HTTP's method. */
export const GRAPHQL_PSEUDO_METHOD = 'GRAPHQL';

/**
 * Stable identity for a graphql entry: `<kind>|<field>` (e.g. `query|order`).
 * Two graphql entries match iff this string is equal (same root field of the
 * same operation kind), the exact-key semantics the Rust side uses. The full
 * canonical key on the Rust side is `graphql|<kind>|<field>`; this is its
 * `<kind>|<field>` tail, which becomes the `path` of the graphql `MatchResult`
 * so the assembled label `GRAPHQL <kind>|<field>` round-trips through the
 * verdict-join (`parse_compat_endpoint` / `parse_producer_key`).
 */
export function graphqlKey(entry: ManifestEntry): string | null {
  if (entry.protocol !== 'graphql' || !entry.kind || !entry.field) {
    return null;
  }
  return `${entry.kind}|${entry.field}`;
}

/**
 * Human label for an entry in orphan/diagnostic strings: `<METHOD> <path>` for
 * HTTP, `socket <DIRECTION>|<event>` for socket, `graphql <kind>|<field>` for
 * graphql.
 */
export function entryLabel(entry: ManifestEntry): string {
  if (entry.protocol === 'socket') {
    return `socket ${socketKey(entry) ?? entry.event ?? '<unknown>'}`;
  }
  if (entry.protocol === 'graphql') {
    return `graphql ${graphqlKey(entry) ?? entry.field ?? '<unknown>'}`;
  }
  return `${entry.method} ${entry.path}`;
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

      // ts_check checks HTTP (by method/path), socket (by event+direction), and
      // graphql (by kind+field) entries with the same assignability checker. Any
      // other protocol is scored by its own pipeline and skipped here. The
      // scanner already filters non-checkable entries out of the manifest files
      // it writes (write_manifest_files), but a stray non-checkable entry must
      // never crash the whole verdict run (#253), so we drop them defensively and
      // leave a trace rather than throwing.
      manifest.entries = this.retainCheckableEntries(manifest.entries, absolutePath);

      // Validate the surviving (HTTP + socket) entries.
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
   * Filter a manifest entry list down to the entries ts_check can check: HTTP
   * (keyed by method/path), socket (keyed by event+direction), and graphql
   * (keyed by kind+field).
   *
   * Any other protocol — or a malformed entry missing the identity its protocol
   * needs — is skipped: it belongs to another pipeline or is junk. A single
   * summary line is logged so the drop is never silent. A stray non-checkable
   * entry can thus never zero out the verdict set (#253).
   */
  private retainCheckableEntries(entries: unknown[], source: string): ManifestEntry[] {
    const retained: ManifestEntry[] = [];
    let skipped = 0;

    for (const entry of entries) {
      if (this.isCheckableManifestEntry(entry)) {
        retained.push(entry);
      } else {
        skipped++;
      }
    }

    if (skipped > 0) {
      console.warn(
        `[manifest] Skipped ${skipped} non-checkable entr${skipped === 1 ? 'y' : 'ies'} in ${source} ` +
          `(not HTTP/socket, or missing its identity fields; other protocols are checked by their own pipelines)`
      );
    }

    return retained;
  }

  /**
   * Shape guard for raw, possibly-malformed manifest JSON. A checkable entry is
   * an HTTP key (`protocol: "http"` with non-empty `method`+`path`), a socket
   * key (`protocol: "socket"` with non-empty `event`+`direction`), or a graphql
   * key (`protocol: "graphql"` with non-empty `kind`+`field`). Everything else
   * (junk) is filtered out here (#253). Full structural validation of the
   * survivors is `validateEntry`'s job — a genuinely-malformed entry still throws.
   */
  private isCheckableManifestEntry(entry: unknown): entry is ManifestEntry {
    if (typeof entry !== 'object' || entry === null) {
      return false;
    }
    const e = entry as Record<string, unknown>;
    if (e.protocol === 'http') {
      return (
        typeof e.method === 'string' &&
        e.method.length > 0 &&
        typeof e.path === 'string' &&
        e.path.length > 0
      );
    }
    if (e.protocol === 'socket') {
      return (
        typeof e.event === 'string' &&
        e.event.length > 0 &&
        (e.direction === 'server_to_client' || e.direction === 'client_to_server')
      );
    }
    if (e.protocol === 'graphql') {
      return (
        typeof e.kind === 'string' &&
        e.kind.length > 0 &&
        typeof e.field === 'string' &&
        e.field.length > 0
      );
    }
    return false;
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

    // Skip non-checkable entries before validating (see loadManifest / #253).
    manifest.entries = this.retainCheckableEntries(manifest.entries, '<string>');

    for (const entry of manifest.entries) {
      this.validateEntry(entry);
    }

    return manifest;
  }

  /**
   * Validate a manifest entry has all required fields.
   *
   * Callers must drop non-checkable entries via `retainCheckableEntries` first;
   * by the time an entry reaches here it is expected to be HTTP or socket. An
   * HTTP entry missing `method`/`path`, or a socket entry missing
   * `event`/`direction`, is a genuine data bug and still throws (the #253
   * guard — a malformed entry must fail loud, not silently pass).
   */
  private validateEntry(entry: ManifestEntry): void {
    if (entry.protocol === 'http') {
      if (!entry.method) {
        throw new Error('ManifestEntry missing required field: method');
      }
      if (!entry.path) {
        throw new Error('ManifestEntry missing required field: path');
      }
    } else if (entry.protocol === 'socket') {
      if (!entry.event) {
        throw new Error('ManifestEntry missing required field: event');
      }
      if (entry.direction !== 'server_to_client' && entry.direction !== 'client_to_server') {
        throw new Error(
          'ManifestEntry missing or invalid field: direction (must be "server_to_client" or "client_to_server")'
        );
      }
    } else if (entry.protocol === 'graphql') {
      if (!entry.kind) {
        throw new Error('ManifestEntry missing required field: kind');
      }
      if (!entry.field) {
        throw new Error('ManifestEntry missing required field: field');
      }
    } else {
      throw new Error('ManifestEntry missing or invalid field: protocol (must be "http", "socket", or "graphql")');
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
    if (!entry.evidence) {
      throw new Error('ManifestEntry missing required field: evidence');
    }
    if (!entry.evidence.file_path) {
      throw new Error('ManifestEntry evidence missing required field: file_path');
    }
    if (typeof entry.evidence.line_number !== 'number') {
      throw new Error('ManifestEntry evidence missing required field: line_number (must be a number)');
    }
    if (!entry.evidence.infer_kind) {
      throw new Error('ManifestEntry evidence missing required field: infer_kind');
    }
    if (typeof entry.evidence.is_explicit !== 'boolean') {
      throw new Error('ManifestEntry evidence missing required field: is_explicit (must be a boolean)');
    }
    if (!entry.evidence.type_state || !['explicit', 'implicit', 'unknown'].includes(entry.evidence.type_state)) {
      throw new Error('ManifestEntry evidence missing or invalid field: type_state (must be "explicit", "implicit", or "unknown")');
    }
    if (
      entry.evidence.span_start !== null &&
      typeof entry.evidence.span_start !== 'number'
    ) {
      throw new Error('ManifestEntry evidence missing or invalid field: span_start (must be a number or null)');
    }
    if (
      entry.evidence.span_end !== null &&
      typeof entry.evidence.span_end !== 'number'
    ) {
      throw new Error('ManifestEntry evidence missing or invalid field: span_end (must be a number or null)');
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

    return manifest.entries.filter((entry) => {
      if (entry.role !== 'producer') return false;
      if (entry.protocol !== 'http') return false;
      if (typeKind && entry.type_kind !== typeKind) return false;

      const entryMethod = normalizeMethod(entry.method!);

      return entryMethod === normalizedMethod && pathsMatch(entry.path!, inputPath);
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

    return manifest.entries.filter((entry) => {
      if (entry.role !== 'consumer') return false;
      if (entry.protocol !== 'http') return false;
      if (typeKind && entry.type_kind !== typeKind) return false;

      const entryMethod = normalizeMethod(entry.method!);

      return entryMethod === normalizedMethod && pathsMatch(entry.path!, inputPath);
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
      if (entry.protocol !== 'http') continue;
      const normalizedMethod = normalizeMethod(entry.method!);
      const normalizedPath = normalizePath(entry.path!);
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

    // HTTP edges: for each consumer, find candidate producers, then keep only
    // the most specific ones. This mirrors routing semantics: a request to
    // /users/me is served by a literal /users/me route when one exists, not by
    // /users/:id — matching both would produce duplicate or contradictory
    // verdicts. Equally specific candidates (e.g. the same route registered
    // by two service versions) are all kept.
    for (let ci = 0; ci < consumerEntries.length; ci++) {
      const consumer = consumerEntries[ci];
      if (consumer.protocol !== 'http') continue;
      const consumerMethod = normalizeMethod(consumer.method!);
      const consumerPath = normalizePath(consumer.path!);

      const candidates: Array<{ pi: number; score: number }> = [];

      for (let pi = 0; pi < producerEntries.length; pi++) {
        const producer = producerEntries[pi];
        if (producer.protocol !== 'http') continue;
        const producerMethod = normalizeMethod(producer.method!);

        if (
          consumerMethod === producerMethod &&
          pathsMatch(consumer.path!, producer.path!) &&
          consumer.type_kind === producer.type_kind
        ) {
          candidates.push({
            pi,
            score: this.calculateMatchScore(producer, consumer),
          });
        }
      }

      if (candidates.length === 0) {
        orphanedConsumers.push({
          entry: consumer,
          reason: `No producer found for ${consumer.method} ${consumer.path} (${consumer.type_kind})`,
        });
        continue;
      }

      const bestScore = Math.max(...candidates.map((c) => c.score));
      for (const candidate of candidates) {
        if (candidate.score !== bestScore) continue;
        const producer = producerEntries[candidate.pi];
        matches.push({
          method: consumerMethod,
          path: consumerPath,
          type_kind: consumer.type_kind,
          producer,
          consumer,
          match_score: candidate.score,
        });
        matchedProducerIndices.add(candidate.pi);
        matchedConsumerIndices.add(ci);
      }
    }

    // Socket edges: exact-key match on `<DIRECTION>|<event>`, the same identity
    // the Rust `analyze_exact_key_matches` uses (no path/route to resolve, so no
    // specificity ranking — a socket consumer matches every producer carrying
    // the same key). The assembled label `SOCKET <DIRECTION>|<event>` round-trips
    // through the Rust verdict-join (`parse_compat_endpoint`/`parse_producer_key`).
    for (let ci = 0; ci < consumerEntries.length; ci++) {
      const consumer = consumerEntries[ci];
      if (consumer.protocol !== 'socket') continue;
      const consumerKey = socketKey(consumer);
      if (consumerKey === null) continue;

      let matched = false;
      for (let pi = 0; pi < producerEntries.length; pi++) {
        const producer = producerEntries[pi];
        if (producer.protocol !== 'socket') continue;
        if (socketKey(producer) !== consumerKey) continue;
        if (producer.type_kind !== consumer.type_kind) continue;

        matches.push({
          method: SOCKET_PSEUDO_METHOD,
          path: consumerKey,
          type_kind: consumer.type_kind,
          producer,
          consumer,
          match_score: 1.0,
        });
        matchedProducerIndices.add(pi);
        matched = true;
      }

      if (matched) {
        matchedConsumerIndices.add(ci);
      } else {
        orphanedConsumers.push({
          entry: consumer,
          reason: `No producer found for socket ${consumerKey} (${consumer.type_kind})`,
        });
      }
    }

    // GraphQL edges: exact-key match on `<kind>|<field>`, the same identity the
    // Rust canonical key carries (`graphql|<kind>|<field>`). Like sockets there
    // is no path/route to resolve, so a graphql consumer matches every producer
    // carrying the same key. The assembled label `GRAPHQL <kind>|<field>`
    // round-trips through the Rust verdict-join
    // (`parse_compat_endpoint` / `parse_producer_key`). The producer/consumer
    // type direction is handled in the type checker (`compareTypes`): the
    // producer payload (SDL field type, structurally unwrapped from the resolver
    // envelope) must satisfy what the consumer reads — `producer ⊑ consumer`,
    // the same data-flow direction as HTTP (server → client), NOT the socket
    // inversion.
    for (let ci = 0; ci < consumerEntries.length; ci++) {
      const consumer = consumerEntries[ci];
      if (consumer.protocol !== 'graphql') continue;
      const consumerKey = graphqlKey(consumer);
      if (consumerKey === null) continue;

      let matched = false;
      for (let pi = 0; pi < producerEntries.length; pi++) {
        const producer = producerEntries[pi];
        if (producer.protocol !== 'graphql') continue;
        if (graphqlKey(producer) !== consumerKey) continue;
        if (producer.type_kind !== consumer.type_kind) continue;

        matches.push({
          method: GRAPHQL_PSEUDO_METHOD,
          path: consumerKey,
          type_kind: consumer.type_kind,
          producer,
          consumer,
          match_score: 1.0,
        });
        matchedProducerIndices.add(pi);
        matched = true;
      }

      if (matched) {
        matchedConsumerIndices.add(ci);
      } else {
        orphanedConsumers.push({
          entry: consumer,
          reason: `No producer found for graphql ${consumerKey} (${consumer.type_kind})`,
        });
      }
    }

    // Find orphaned producers (producers with no matching consumers)
    for (let pi = 0; pi < producerEntries.length; pi++) {
      if (!matchedProducerIndices.has(pi)) {
        const producer = producerEntries[pi];
        orphanedProducers.push({
          entry: producer,
          reason: `No consumer found for ${this.entryLabel(producer)} (${producer.type_kind})`,
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
   * Human label for an entry in orphan/diagnostic strings. Delegates to the
   * module-level `entryLabel` so the matcher and the type-checker format
   * orphans identically.
   */
  private entryLabel(entry: ManifestEntry): string {
    return entryLabel(entry);
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
    // Only ever called from the HTTP matching loop, so both paths are present.
    const norm1 = normalizePath(producer.path!);
    const norm2 = normalizePath(consumer.path!);

    // Exact normalized match (both parameterized or both identical)
    if (norm1 === norm2) {
      return producer.path === consumer.path ? 1.0 : 0.95;
    }

    // Segment-level match (concrete value matched against parameter)
    return 0.9;
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
  typeState: ManifestTypeState = isExplicit ? 'explicit' : 'implicit',
  evidenceOverrides: Partial<TypeEvidence> = {}
): ManifestEntry {
  const inferKind =
    role === 'consumer' && typeKind === 'response'
      ? 'call_result'
      : typeKind === 'request'
        ? 'request_body'
        : 'response_body';
  const evidence: TypeEvidence = {
    file_path: filePath,
    span_start: null,
    span_end: null,
    line_number: lineNumber,
    infer_kind: inferKind,
    is_explicit: isExplicit,
    type_state: typeState,
    ...evidenceOverrides,
  };

  return {
    protocol: 'http',
    method: normalizeMethod(method),
    path: entryPath,
    type_alias: typeAlias,
    role,
    type_kind: typeKind,
    file_path: filePath,
    line_number: lineNumber,
    is_explicit: isExplicit,
    type_state: typeState,
    evidence,
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
