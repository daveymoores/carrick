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
        repo_name: 'test-repo',
        commit_hash: 'abc123',
        entries: [
          {
            method: 'GET',
            path: '/api/users',
            type_alias: 'GetUsersResponse',
            role: 'producer',
            file_path: 'src/routes.ts',
            line_number: 10,
          },
        ],
      };

      const filePath = path.join(tempDir, 'manifest.json');
      fs.writeFileSync(filePath, JSON.stringify(manifest));

      const loaded = matcher.loadManifest(filePath);
      assert.strictEqual(loaded.repo_name, 'test-repo');
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

    it('should throw error for invalid entry', () => {
      const filePath = path.join(tempDir, 'invalid-entry.json');
      fs.writeFileSync(
        filePath,
        JSON.stringify({
          repo_name: 'test',
          commit_hash: 'abc123',
          entries: [{ method: 'GET' }], // Missing required fields
        })
      );

      assert.throws(() => matcher.loadManifest(filePath), /missing required field/);
    });
  });

  describe('parseManifest', () => {
    it('should parse valid JSON string', () => {
      const json = JSON.stringify({
        repo_name: 'test-repo',
        commit_hash: 'abc123',
        entries: [],
      });

      const manifest = matcher.parseManifest(json);
      assert.strictEqual(manifest.repo_name, 'test-repo');
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
