/**
 * Tests for ManifestMatcher
 *
 * Tests cover:
 * - Path normalization (trailing slashes, parameter formats, case)
 * - Manifest loading and parsing
 * - Producer/consumer endpoint matching
 * - Orphaned entry detection
 */

import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';
import {
  ManifestMatcher,
  TypeManifest,
  ManifestEntry,
  normalizePath,
  pathsMatch,
  normalizeMethod,
  createManifestEntry,
  mergeManifests,
} from './manifest-matcher';

// ============================================================================
// Path Normalization Tests
// ============================================================================

describe('normalizePath', () => {
  it('should handle trailing slashes', () => {
    assert.strictEqual(normalizePath('/api/users/'), '/api/users');
    assert.strictEqual(normalizePath('/api/users'), '/api/users');
    assert.strictEqual(normalizePath('/api/users///'), '/api/users');
  });

  it('should ensure leading slash', () => {
    assert.strictEqual(normalizePath('api/users'), '/api/users');
    assert.strictEqual(normalizePath('/api/users'), '/api/users');
  });

  it('should normalize multiple slashes', () => {
    assert.strictEqual(normalizePath('/api//users'), '/api/users');
    assert.strictEqual(normalizePath('///api///users///'), '/api/users');
  });

  it('should convert to lowercase', () => {
    assert.strictEqual(normalizePath('/API/Users'), '/api/users');
    assert.strictEqual(normalizePath('/Api/USERS'), '/api/users');
  });

  it('should normalize Express-style path parameters (:id)', () => {
    assert.strictEqual(normalizePath('/api/users/:id'), '/api/users/:param');
    assert.strictEqual(normalizePath('/api/users/:userId'), '/api/users/:param');
    assert.strictEqual(normalizePath('/api/users/:user_id'), '/api/users/:param');
  });

  it('should normalize OpenAPI-style path parameters ({id})', () => {
    assert.strictEqual(normalizePath('/api/users/{id}'), '/api/users/:param');
    assert.strictEqual(normalizePath('/api/users/{userId}'), '/api/users/:param');
  });

  it('should normalize Next.js-style path parameters ([id])', () => {
    assert.strictEqual(normalizePath('/api/users/[id]'), '/api/users/:param');
    assert.strictEqual(normalizePath('/api/users/[userId]'), '/api/users/:param');
  });

  it('should handle multiple path parameters', () => {
    assert.strictEqual(
      normalizePath('/api/users/:userId/posts/:postId'),
      '/api/users/:param/posts/:param'
    );
    assert.strictEqual(
      normalizePath('/api/users/{userId}/posts/{postId}'),
      '/api/users/:param/posts/:param'
    );
  });

  it('should handle mixed parameter formats', () => {
    // This is an edge case - different formats in same path
    assert.strictEqual(
      normalizePath('/api/users/:id/posts/{postId}'),
      '/api/users/:param/posts/:param'
    );
  });
});

describe('pathsMatch', () => {
  it('should match numeric segments against params', () => {
    assert.ok(pathsMatch('/api/orders/:id', '/api/orders/101'));
  });

  it('should match non-numeric segments against params (UUIDs, slugs)', () => {
    assert.ok(pathsMatch('/api/orders/:id', '/api/orders/abc-123'));
    assert.ok(pathsMatch('/api/orders/:id', '/api/orders/550e8400-e29b-41d4'));
  });

  it('should match different param names', () => {
    assert.ok(pathsMatch('/users/:id', '/users/:userId'));
    assert.ok(pathsMatch('/users/:id/comments', '/users/:userId/comments'));
  });

  it('should not match different segment counts', () => {
    assert.ok(!pathsMatch('/api/orders', '/api/orders/101'));
  });

  it('should not match different literal segments', () => {
    assert.ok(!pathsMatch('/api/users', '/api/orders'));
  });

  it('should handle template literal remnants', () => {
    assert.ok(pathsMatch('/membership/:param', '/membership/gold'));
  });
});

describe('normalizeMethod', () => {
  it('should convert to uppercase', () => {
    assert.strictEqual(normalizeMethod('get'), 'GET');
    assert.strictEqual(normalizeMethod('post'), 'POST');
    assert.strictEqual(normalizeMethod('GET'), 'GET');
    assert.strictEqual(normalizeMethod('Post'), 'POST');
  });
});

// ============================================================================
// ManifestMatcher Tests
// ============================================================================

describe('ManifestMatcher', () => {
  let matcher: ManifestMatcher;
  let tempDir: string;

  beforeEach(() => {
    matcher = new ManifestMatcher();
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'manifest-matcher-test-'));
  });

  afterEach(() => {
    // Clean up temp directory
    if (fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true });
    }
  });

  // --------------------------------------------------------------------------
  // Manifest Loading Tests
  // --------------------------------------------------------------------------

  describe('loadManifest', () => {
    it('should load a valid manifest file', () => {
      const manifest: TypeManifest = {
        repo_name: 'express-single',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'GetUsersResponse',
            'producer',
            'src/routes.ts',
            10
          ),
        ],
      };

      const filePath = path.join(tempDir, 'manifest.json');
      fs.writeFileSync(filePath, JSON.stringify(manifest));

      const loaded = matcher.loadManifest(filePath);
      assert.strictEqual(loaded.repo_name, 'express-single');
      assert.strictEqual(loaded.commit_hash, 'abc123');
      assert.strictEqual(loaded.entries.length, 1);
    });

    it('should throw error for non-existent file', () => {
      assert.throws(
        () => matcher.loadManifest('/nonexistent/path/manifest.json'),
        /Manifest file not found/
      );
    });

    it('should throw error for invalid JSON', () => {
      const filePath = path.join(tempDir, 'invalid.json');
      fs.writeFileSync(filePath, 'not valid json {{{');

      assert.throws(() => matcher.loadManifest(filePath), /Invalid JSON/);
    });

    it('should throw error for missing repo_name', () => {
      const filePath = path.join(tempDir, 'missing-repo.json');
      fs.writeFileSync(
        filePath,
        JSON.stringify({
          commit_hash: 'abc123',
          entries: [],
        })
      );

      assert.throws(
        () => matcher.loadManifest(filePath),
        /missing required field: repo_name/
      );
    });

    it('should throw error for missing commit_hash', () => {
      const filePath = path.join(tempDir, 'missing-commit.json');
      fs.writeFileSync(
        filePath,
        JSON.stringify({
          repo_name: 'test',
          entries: [],
        })
      );

      assert.throws(
        () => matcher.loadManifest(filePath),
        /missing required field: commit_hash/
      );
    });

    it('should throw error for a structurally-invalid HTTP entry', () => {
      const filePath = path.join(tempDir, 'invalid-entry.json');
      // An HTTP entry (protocol/method/path present) missing other required
      // fields is a genuine data bug and must still throw — it is not a
      // skippable non-HTTP entry.
      fs.writeFileSync(
        filePath,
        JSON.stringify({
          repo_name: 'test',
          commit_hash: 'abc123',
          entries: [{ protocol: 'http', method: 'GET', path: '/api/users' }],
        })
      );

      assert.throws(() => matcher.loadManifest(filePath), /missing required field/);
    });

    // --- #253 regression: non-HTTP entries must be skipped, not fatal ---

    it('should keep HTTP, socket, and graphql entries and drop a non-checkable protocol', () => {
      const httpEntry = createManifestEntry(
        'GET',
        '/orders/:id',
        'OrderResponse',
        'producer',
        'src/routes.ts',
        10
      );
      // GraphQL (kind+field) is now checked alongside HTTP and socket. A truly
      // non-checkable entry (an unrecognised protocol) is still dropped, and must
      // never throw in validateEntry (#253).
      const graphqlEntry = {
        protocol: 'graphql',
        kind: 'query',
        field: 'order',
        type_alias: 'OrderQueryResult',
        role: 'producer',
        type_kind: 'response',
        file_path: 'src/schema.ts',
        line_number: 5,
        is_explicit: true,
        type_state: 'explicit',
        evidence: {
          file_path: 'src/schema.ts',
          span_start: null,
          span_end: null,
          line_number: 5,
          infer_kind: 'response_body',
          is_explicit: true,
          type_state: 'explicit',
        },
      };
      const socketEntry = {
        protocol: 'socket',
        event: 'order.created',
        direction: 'server_to_client',
        type_alias: 'OrderCreatedEvent',
        role: 'producer',
        type_kind: 'response',
        file_path: 'src/sockets.ts',
        line_number: 7,
        is_explicit: true,
        type_state: 'explicit',
        evidence: {
          file_path: 'src/sockets.ts',
          span_start: null,
          span_end: null,
          line_number: 7,
          infer_kind: 'response_body',
          is_explicit: true,
          type_state: 'explicit',
        },
      };
      // Unrecognised protocol — not HTTP/socket/graphql — must be dropped (#253).
      const nonCheckableEntry = {
        protocol: 'grpc',
        type_alias: 'SomeGrpcResult',
        role: 'producer',
        type_kind: 'response',
        file_path: 'src/grpc.ts',
        line_number: 9,
        is_explicit: true,
        type_state: 'explicit',
        evidence: {
          file_path: 'src/grpc.ts',
          span_start: null,
          span_end: null,
          line_number: 9,
          infer_kind: 'response_body',
          is_explicit: true,
          type_state: 'explicit',
        },
      };

      const filePath = path.join(tempDir, 'mixed-protocol.json');
      fs.writeFileSync(
        filePath,
        JSON.stringify({
          repo_name: 'orders-svc',
          commit_hash: 'abc123',
          entries: [graphqlEntry, httpEntry, socketEntry, nonCheckableEntry],
        })
      );

      // Must NOT throw. Keeps the HTTP + socket + graphql entries, drops grpc.
      const loaded = matcher.loadManifest(filePath);
      assert.strictEqual(loaded.entries.length, 3);
      assert.ok(
        loaded.entries.some((e) => e.protocol === 'http' && e.path === '/orders/:id'),
        'HTTP entry must survive'
      );
      assert.ok(
        loaded.entries.some(
          (e) => e.protocol === 'socket' && e.event === 'order.created'
        ),
        'socket entry must survive'
      );
      assert.ok(
        loaded.entries.some(
          (e) => e.protocol === 'graphql' && e.kind === 'query' && e.field === 'order'
        ),
        'graphql entry must survive'
      );
      // Length 3 (from 4 inputs) with the survivors above proves the grpc entry
      // was dropped. Assert the surviving protocols are exactly the checkable ones.
      assert.deepStrictEqual(
        loaded.entries.map((e) => e.protocol).sort(),
        ['graphql', 'http', 'socket']
      );
    });

    it('should still match HTTP edges when a non-HTTP entry is present', () => {
      const producerPath = path.join(tempDir, 'producer.json');
      const consumerPath = path.join(tempDir, 'consumer.json');

      const graphqlEntry = {
        protocol: 'graphql',
        kind: 'query',
        field: 'order',
        type_alias: 'OrderQueryResult',
        role: 'producer',
        type_kind: 'response',
        file_path: 'src/schema.ts',
        line_number: 5,
        is_explicit: true,
        type_state: 'explicit',
        evidence: {
          file_path: 'src/schema.ts',
          span_start: null,
          span_end: null,
          line_number: 5,
          infer_kind: 'response_body',
          is_explicit: true,
          type_state: 'explicit',
        },
      };

      fs.writeFileSync(
        producerPath,
        JSON.stringify({
          repo_name: 'orders-svc',
          commit_hash: 'abc123',
          entries: [
            graphqlEntry,
            createManifestEntry('GET', '/orders/:id', 'OrderResponse', 'producer', 'src/routes.ts', 10),
          ],
        })
      );
      fs.writeFileSync(
        consumerPath,
        JSON.stringify({
          repo_name: 'web-frontend',
          commit_hash: 'def456',
          entries: [
            createManifestEntry('GET', '/orders/:id', 'OrderData', 'consumer', 'src/api.ts', 20),
          ],
        })
      );

      const producers = matcher.loadManifest(producerPath);
      const consumers = matcher.loadManifest(consumerPath);

      const result = matcher.matchEndpoints(producers, consumers);
      assert.strictEqual(result.matches.length, 1);
      assert.strictEqual(result.matches[0].producer.path, '/orders/:id');
      assert.strictEqual(result.matches[0].consumer.path, '/orders/:id');
    });
  });

  describe('parseManifest', () => {
    it('should parse valid JSON string', () => {
      const json = JSON.stringify({
        repo_name: 'express-single',
        commit_hash: 'abc123',
        entries: [],
      });

      const manifest = matcher.parseManifest(json);
      assert.strictEqual(manifest.repo_name, 'express-single');
    });
  });

  // --------------------------------------------------------------------------
  // Endpoint Finding Tests
  // --------------------------------------------------------------------------

  describe('findProducersForEndpoint', () => {
    it('should find matching producer entries', () => {
      const manifest: TypeManifest = {
        repo_name: 'api-service',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry('GET', '/api/users', 'GetUsersResponse', 'producer', 'routes.ts', 10),
          createManifestEntry('POST', '/api/users', 'CreateUserResponse', 'producer', 'routes.ts', 20),
          createManifestEntry('GET', '/api/users', 'GetUsersRequest', 'consumer', 'client.ts', 30),
        ],
      };

      const producers = matcher.findProducersForEndpoint(manifest, 'GET', '/api/users');
      assert.strictEqual(producers.length, 1);
      assert.strictEqual(producers[0].type_alias, 'GetUsersResponse');
    });

    it('should match with normalized paths', () => {
      const manifest: TypeManifest = {
        repo_name: 'api-service',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry('GET', '/api/users/:id', 'GetUserResponse', 'producer', 'routes.ts', 10),
        ],
      };

      // Different path param formats should match
      const producers1 = matcher.findProducersForEndpoint(manifest, 'get', '/api/users/{userId}');
      assert.strictEqual(producers1.length, 1);

      const producers2 = matcher.findProducersForEndpoint(manifest, 'GET', '/api/users/[id]');
      assert.strictEqual(producers2.length, 1);

      const producers3 = matcher.findProducersForEndpoint(manifest, 'GET', '/api/users/:order.userId');
      assert.strictEqual(producers3.length, 1);
    });

    it('should match numeric path segments to params', () => {
      const manifest: TypeManifest = {
        repo_name: 'api-service',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry('GET', '/api/users/:id', 'GetUserResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const producers = matcher.findProducersForEndpoint(manifest, 'GET', '/api/users/123');
      assert.strictEqual(producers.length, 1);
    });

    it('should match non-numeric path segments (UUIDs, slugs) to params', () => {
      const manifest: TypeManifest = {
        repo_name: 'api-service',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry('GET', '/api/users/:id', 'GetUserResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const uuidProducers = matcher.findProducersForEndpoint(manifest, 'GET', '/api/users/550e8400-e29b-41d4');
      assert.strictEqual(uuidProducers.length, 1);

      const slugProducers = matcher.findProducersForEndpoint(manifest, 'GET', '/api/users/abc-123');
      assert.strictEqual(slugProducers.length, 1);
    });

    it('should return empty array for no matches', () => {
      const manifest: TypeManifest = {
        repo_name: 'api-service',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry('GET', '/api/users', 'GetUsersResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const producers = matcher.findProducersForEndpoint(manifest, 'POST', '/api/users');
      assert.strictEqual(producers.length, 0);
    });
  });

  describe('findConsumersForEndpoint', () => {
    it('should find matching consumer entries', () => {
      const manifest: TypeManifest = {
        repo_name: 'frontend',
        commit_hash: 'def456',
        entries: [
          createManifestEntry('GET', '/api/users', 'GetUsersResponse', 'consumer', 'api.ts', 10),
          createManifestEntry('POST', '/api/users', 'CreateUserResponse', 'consumer', 'api.ts', 20),
          createManifestEntry('GET', '/api/users', 'GetUsersResponse', 'producer', 'server.ts', 30),
        ],
      };

      const consumers = matcher.findConsumersForEndpoint(manifest, 'GET', '/api/users');
      assert.strictEqual(consumers.length, 1);
      assert.strictEqual(consumers[0].type_alias, 'GetUsersResponse');
      assert.strictEqual(consumers[0].role, 'consumer');
    });
  });

  describe('getUniqueEndpoints', () => {
    it('should return unique method+path combinations', () => {
      const manifest: TypeManifest = {
        repo_name: 'test',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry('GET', '/api/users', 'Type1', 'producer', 'a.ts', 1),
          createManifestEntry('GET', '/api/users', 'Type2', 'consumer', 'b.ts', 2),
          createManifestEntry('POST', '/api/users', 'Type3', 'producer', 'c.ts', 3),
          createManifestEntry('GET', '/API/Users/', 'Type4', 'producer', 'd.ts', 4), // Same as first after normalization
        ],
      };

      const endpoints = matcher.getUniqueEndpoints(manifest);
      assert.strictEqual(endpoints.length, 2);
      assert.ok(endpoints.some((e) => e.method === 'GET' && e.path === '/api/users'));
      assert.ok(endpoints.some((e) => e.method === 'POST' && e.path === '/api/users'));
    });
  });

  // --------------------------------------------------------------------------
  // Endpoint Matching Tests
  // --------------------------------------------------------------------------

  describe('matchEndpoints', () => {
    it('should match producers and consumers with same method and path', () => {
      const producers: TypeManifest = {
        repo_name: 'api-service',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry('GET', '/api/users', 'GetUsersResponse', 'producer', 'routes.ts', 10),
          createManifestEntry('POST', '/api/users', 'CreateUserResponse', 'producer', 'routes.ts', 20),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'frontend',
        commit_hash: 'def456',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersData', 'consumer', 'api.ts', 5),
        ],
      };

      const result = matcher.matchEndpoints(producers, consumers);

      assert.strictEqual(result.matches.length, 1);
      assert.strictEqual(result.matches[0].method, 'GET');
      assert.strictEqual(result.matches[0].producer.type_alias, 'GetUsersResponse');
      assert.strictEqual(result.matches[0].consumer.type_alias, 'UsersData');

      assert.strictEqual(result.orphanedProducers.length, 1);
      assert.strictEqual(result.orphanedProducers[0].entry.type_alias, 'CreateUserResponse');

      assert.strictEqual(result.orphanedConsumers.length, 0);
    });

    it('should match with path parameter normalization', () => {
      const producers: TypeManifest = {
        repo_name: 'api-service',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry('GET', '/api/users/:id', 'GetUserResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'frontend',
        commit_hash: 'def456',
        entries: [
          createManifestEntry('GET', '/api/users/{userId}', 'UserData', 'consumer', 'api.ts', 5),
        ],
      };

      const result = matcher.matchEndpoints(producers, consumers);

      assert.strictEqual(result.matches.length, 1);
      assert.strictEqual(result.orphanedProducers.length, 0);
      assert.strictEqual(result.orphanedConsumers.length, 0);
    });

    it('should identify orphaned consumers', () => {
      const producers: TypeManifest = {
        repo_name: 'api-service',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry('GET', '/api/users', 'GetUsersResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'frontend',
        commit_hash: 'def456',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersData', 'consumer', 'api.ts', 5),
          createManifestEntry('DELETE', '/api/users/:id', 'DeleteResult', 'consumer', 'api.ts', 15),
        ],
      };

      const result = matcher.matchEndpoints(producers, consumers);

      assert.strictEqual(result.matches.length, 1);
      assert.strictEqual(result.orphanedProducers.length, 0);
      assert.strictEqual(result.orphanedConsumers.length, 1);
      assert.strictEqual(result.orphanedConsumers[0].entry.type_alias, 'DeleteResult');
      assert.ok(result.orphanedConsumers[0].reason.includes('No producer found'));
    });

    it('should handle multiple consumers for same endpoint', () => {
      const producers: TypeManifest = {
        repo_name: 'api-service',
        commit_hash: 'abc123',
        entries: [
          createManifestEntry('GET', '/api/users', 'GetUsersResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'frontend',
        commit_hash: 'def456',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersListData', 'consumer', 'users.ts', 5),
          createManifestEntry('GET', '/api/users', 'AdminUsersData', 'consumer', 'admin.ts', 15),
        ],
      };

      const result = matcher.matchEndpoints(producers, consumers);

      // Both consumers should match the same producer
      assert.strictEqual(result.matches.length, 2);
      assert.ok(result.matches.every((m) => m.producer.type_alias === 'GetUsersResponse'));
      assert.strictEqual(result.orphanedProducers.length, 0);
      assert.strictEqual(result.orphanedConsumers.length, 0);
    });

    it('should handle empty manifests', () => {
      const producers: TypeManifest = {
        repo_name: 'api-service',
        commit_hash: 'abc123',
        entries: [],
      };

      const consumers: TypeManifest = {
        repo_name: 'frontend',
        commit_hash: 'def456',
        entries: [],
      };

      const result = matcher.matchEndpoints(producers, consumers);

      assert.strictEqual(result.matches.length, 0);
      assert.strictEqual(result.orphanedProducers.length, 0);
      assert.strictEqual(result.orphanedConsumers.length, 0);
    });

    it('should match a socket producer and consumer on the canonical key', () => {
      const socketEntry = (
        typeAlias: string,
        role: 'producer' | 'consumer'
      ): ManifestEntry => ({
        protocol: 'socket',
        event: 'payment:settled',
        direction: 'server_to_client',
        type_alias: typeAlias,
        role,
        type_kind: 'response',
        file_path: role === 'producer' ? 'lib/realtime.ts' : 'realtime/server.ts',
        line_number: 31,
        is_explicit: true,
        type_state: 'explicit',
        evidence: {
          file_path: role === 'producer' ? 'lib/realtime.ts' : 'realtime/server.ts',
          span_start: null,
          span_end: null,
          line_number: 31,
          infer_kind: 'response_body',
          is_explicit: true,
          type_state: 'explicit',
        },
      });

      const producers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'abc123',
        entries: [socketEntry('SettledPayment', 'producer')],
      };
      const consumers: TypeManifest = {
        repo_name: 'payments-svc',
        commit_hash: 'def456',
        entries: [socketEntry('Payment', 'consumer')],
      };

      const result = matcher.matchEndpoints(producers, consumers);

      assert.strictEqual(result.matches.length, 1);
      assert.strictEqual(result.matches[0].method, 'SOCKET');
      assert.strictEqual(result.matches[0].path, 'SERVER->CLIENT|payment:settled');
      assert.strictEqual(result.matches[0].producer.type_alias, 'SettledPayment');
      assert.strictEqual(result.matches[0].consumer.type_alias, 'Payment');
      assert.strictEqual(result.orphanedProducers.length, 0);
      assert.strictEqual(result.orphanedConsumers.length, 0);
    });

    it('should not match socket entries flowing in different directions', () => {
      const entry = (
        direction: 'server_to_client' | 'client_to_server',
        role: 'producer' | 'consumer'
      ): ManifestEntry => ({
        protocol: 'socket',
        event: 'payment:settled',
        direction,
        type_alias: 'Payment',
        role,
        type_kind: 'response',
        file_path: 'f.ts',
        line_number: 1,
        is_explicit: true,
        type_state: 'explicit',
        evidence: {
          file_path: 'f.ts',
          span_start: null,
          span_end: null,
          line_number: 1,
          infer_kind: 'response_body',
          is_explicit: true,
          type_state: 'explicit',
        },
      });

      const producers: TypeManifest = {
        repo_name: 'a',
        commit_hash: 'x',
        entries: [entry('server_to_client', 'producer')],
      };
      const consumers: TypeManifest = {
        repo_name: 'b',
        commit_hash: 'y',
        entries: [entry('client_to_server', 'consumer')],
      };

      const result = matcher.matchEndpoints(producers, consumers);

      assert.strictEqual(result.matches.length, 0);
      assert.strictEqual(result.orphanedProducers.length, 1);
      assert.strictEqual(result.orphanedConsumers.length, 1);
    });

    it('should match a graphql producer and consumer on the canonical kind|field key', () => {
      const graphqlEntry = (
        typeAlias: string,
        role: 'producer' | 'consumer'
      ): ManifestEntry => ({
        protocol: 'graphql',
        kind: 'query',
        field: 'order',
        type_alias: typeAlias,
        role,
        type_kind: 'response',
        file_path: role === 'producer' ? 'gateway/src/orders.resolver.ts' : 'lib/graphql.ts',
        line_number: role === 'producer' ? 40 : 76,
        is_explicit: true,
        type_state: 'explicit',
        evidence: {
          file_path: role === 'producer' ? 'gateway/src/orders.resolver.ts' : 'lib/graphql.ts',
          span_start: null,
          span_end: null,
          line_number: role === 'producer' ? 40 : 76,
          infer_kind: 'response_body',
          is_explicit: true,
          type_state: 'explicit',
        },
      });

      const producers: TypeManifest = {
        repo_name: 'orders-monorepo',
        commit_hash: 'abc123',
        entries: [graphqlEntry('ProducerEnvelope', 'producer')],
      };
      const consumers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'def456',
        entries: [graphqlEntry('OrderView', 'consumer')],
      };

      const result = matcher.matchEndpoints(producers, consumers);

      assert.strictEqual(result.matches.length, 1);
      assert.strictEqual(result.matches[0].method, 'GRAPHQL');
      assert.strictEqual(result.matches[0].path, 'query|order');
      assert.strictEqual(result.matches[0].producer.type_alias, 'ProducerEnvelope');
      assert.strictEqual(result.matches[0].consumer.type_alias, 'OrderView');
      assert.strictEqual(result.orphanedProducers.length, 0);
      assert.strictEqual(result.orphanedConsumers.length, 0);
    });

    it('should not match graphql entries with different fields', () => {
      const entry = (
        field: string,
        role: 'producer' | 'consumer'
      ): ManifestEntry => ({
        protocol: 'graphql',
        kind: 'query',
        field,
        type_alias: 'OrderView',
        role,
        type_kind: 'response',
        file_path: 'f.ts',
        line_number: 1,
        is_explicit: true,
        type_state: 'explicit',
        evidence: {
          file_path: 'f.ts',
          span_start: null,
          span_end: null,
          line_number: 1,
          infer_kind: 'response_body',
          is_explicit: true,
          type_state: 'explicit',
        },
      });

      const producers: TypeManifest = {
        repo_name: 'a',
        commit_hash: 'x',
        entries: [entry('orders', 'producer')],
      };
      const consumers: TypeManifest = {
        repo_name: 'b',
        commit_hash: 'y',
        entries: [entry('order', 'consumer')],
      };

      const result = matcher.matchEndpoints(producers, consumers);

      assert.strictEqual(result.matches.length, 0);
      assert.strictEqual(result.orphanedProducers.length, 1);
      assert.strictEqual(result.orphanedConsumers.length, 1);
    });
  });

  // --------------------------------------------------------------------------
  // Manifest Creation and Serialization Tests
  // --------------------------------------------------------------------------

  describe('createEmptyManifest', () => {
    it('should create an empty manifest with correct fields', () => {
      const manifest = matcher.createEmptyManifest('my-repo', 'sha256hash');

      assert.strictEqual(manifest.repo_name, 'my-repo');
      assert.strictEqual(manifest.commit_hash, 'sha256hash');
      assert.deepStrictEqual(manifest.entries, []);
    });
  });

  describe('addEntry', () => {
    it('should add valid entry to manifest', () => {
      const manifest = matcher.createEmptyManifest('test', 'abc');
      const entry = createManifestEntry('GET', '/api/test', 'TestType', 'producer', 'test.ts', 1);

      matcher.addEntry(manifest, entry);

      assert.strictEqual(manifest.entries.length, 1);
      assert.strictEqual(manifest.entries[0].type_alias, 'TestType');
    });
  });

  describe('serializeManifest', () => {
    it('should serialize manifest to JSON', () => {
      const manifest: TypeManifest = {
        repo_name: 'test',
        commit_hash: 'abc',
        entries: [],
      };

      const json = matcher.serializeManifest(manifest);
      const parsed = JSON.parse(json);

      assert.strictEqual(parsed.repo_name, 'test');
    });

    it('should support compact JSON', () => {
      const manifest = matcher.createEmptyManifest('test', 'abc');
      const compact = matcher.serializeManifest(manifest, false);

      assert.ok(!compact.includes('\n'));
    });
  });

  describe('saveManifest', () => {
    it('should save manifest to file', () => {
      const manifest = matcher.createEmptyManifest('saved-repo', 'xyz');
      const filePath = path.join(tempDir, 'saved-manifest.json');

      matcher.saveManifest(manifest, filePath);

      assert.ok(fs.existsSync(filePath));
      const loaded = matcher.loadManifest(filePath);
      assert.strictEqual(loaded.repo_name, 'saved-repo');
    });
  });
});

// ============================================================================
// Utility Function Tests
// ============================================================================

describe('createManifestEntry', () => {
  it('should create entry with normalized method', () => {
    const entry = createManifestEntry('get', '/api/users', 'Type', 'producer', 'file.ts', 10);

    assert.strictEqual(entry.method, 'GET');
    assert.strictEqual(entry.evidence.file_path, 'file.ts');
    assert.strictEqual(entry.evidence.line_number, 10);
  });

  it('should preserve original path', () => {
    const entry = createManifestEntry('POST', '/api/users/:id', 'Type', 'producer', 'file.ts', 10);

    assert.strictEqual(entry.path, '/api/users/:id');
  });
});

describe('mergeManifests', () => {
  it('should merge entries from multiple manifests', () => {
    const manifest1: TypeManifest = {
      repo_name: 'repo1',
      commit_hash: 'abc',
      entries: [
        createManifestEntry('GET', '/api/a', 'TypeA', 'producer', 'a.ts', 1),
      ],
    };

    const manifest2: TypeManifest = {
      repo_name: 'repo2',
      commit_hash: 'def',
      entries: [
        createManifestEntry('GET', '/api/b', 'TypeB', 'producer', 'b.ts', 1),
        createManifestEntry('GET', '/api/c', 'TypeC', 'producer', 'c.ts', 1),
      ],
    };

    const merged = mergeManifests([manifest1, manifest2], 'merged', 'xyz');

    assert.strictEqual(merged.repo_name, 'merged');
    assert.strictEqual(merged.commit_hash, 'xyz');
    assert.strictEqual(merged.entries.length, 3);
  });

  it('should handle empty input', () => {
    const merged = mergeManifests([], 'empty', 'hash');

    assert.strictEqual(merged.entries.length, 0);
  });
});
