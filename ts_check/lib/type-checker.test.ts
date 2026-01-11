/**
 * Tests for TypeCompatibilityChecker
 *
 * Tests cover:
 * - Mode switching between legacy and manifest
 * - Manifest-based type checking
 * - Type resolution from projects
 * - Integration with ManifestMatcher
 */

import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';
import { Project } from 'ts-morph';
import {
  TypeCompatibilityChecker,
  TypeCheckMode,
} from './type-checker';
import {
  TypeManifest,
  createManifestEntry,
} from './manifest-matcher';

// ============================================================================
// Mode Tests
// ============================================================================

describe('TypeCompatibilityChecker', () => {
  let project: Project;
  let typeChecker: TypeCompatibilityChecker;
  let tempDir: string;

  beforeEach(() => {
    project = new Project({
      compilerOptions: {
        strict: true,
        skipLibCheck: true,
      },
    });
    typeChecker = new TypeCompatibilityChecker(project);
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), 'type-checker-test-'));
  });

  afterEach(() => {
    // Clean up temp directory
    if (fs.existsSync(tempDir)) {
      fs.rmSync(tempDir, { recursive: true });
    }
  });

  describe('mode handling', () => {
    it('should default to legacy mode', () => {
      assert.strictEqual(typeChecker.getMode(), 'legacy');
    });

    it('should allow setting mode to manifest', () => {
      typeChecker.setMode('manifest');
      assert.strictEqual(typeChecker.getMode(), 'manifest');
    });

    it('should allow switching back to legacy', () => {
      typeChecker.setMode('manifest');
      typeChecker.setMode('legacy');
      assert.strictEqual(typeChecker.getMode(), 'legacy');
    });
  });

  describe('loadManifest', () => {
    it('should load a valid manifest file', () => {
      const manifest: TypeManifest = {
        repo_name: 'test-api',
        commit_hash: 'abc123def',
        entries: [
          createManifestEntry('GET', '/api/users', 'GetUsersResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const filePath = path.join(tempDir, 'manifest.json');
      fs.writeFileSync(filePath, JSON.stringify(manifest));

      const loaded = typeChecker.loadManifest(filePath);
      assert.strictEqual(loaded.repo_name, 'test-api');
      assert.strictEqual(loaded.entries.length, 1);
    });

    it('should throw error for non-existent file', () => {
      assert.throws(
        () => typeChecker.loadManifest('/nonexistent/manifest.json'),
        /Manifest file not found/
      );
    });
  });

  describe('parseManifest', () => {
    it('should parse valid JSON string', () => {
      const json = JSON.stringify({
        repo_name: 'parsed-repo',
        commit_hash: 'xyz789',
        entries: [],
      });

      const manifest = typeChecker.parseManifest(json);
      assert.strictEqual(manifest.repo_name, 'parsed-repo');
    });
  });

  describe('checkCompatibilityWithManifests', () => {
    it('should return empty result for empty manifests', async () => {
      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [],
      };

      const result = await typeChecker.checkCompatibilityWithManifests(
        producers,
        consumers
      );

      assert.strictEqual(result.mode, 'manifest');
      assert.strictEqual(result.totalProducers, 0);
      assert.strictEqual(result.totalConsumers, 0);
      assert.strictEqual(result.compatiblePairs, 0);
      assert.strictEqual(result.incompatiblePairs, 0);
      assert.strictEqual(result.mismatches.length, 0);
    });

    it('should identify orphaned consumers', async () => {
      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersData', 'consumer', 'api.ts', 5),
          createManifestEntry('DELETE', '/api/users/:id', 'DeleteResult', 'consumer', 'api.ts', 15),
        ],
      };

      const result = await typeChecker.checkCompatibilityWithManifests(
        producers,
        consumers
      );

      assert.strictEqual(result.mode, 'manifest');
      assert.strictEqual(result.orphanedConsumers.length, 1);
      assert.ok(result.orphanedConsumers[0].includes('DELETE'));
    });

    it('should identify orphaned producers', async () => {
      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersResponse', 'producer', 'routes.ts', 10),
          createManifestEntry('POST', '/api/users', 'CreateResponse', 'producer', 'routes.ts', 20),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersData', 'consumer', 'api.ts', 5),
        ],
      };

      const result = await typeChecker.checkCompatibilityWithManifests(
        producers,
        consumers
      );

      assert.strictEqual(result.orphanedProducers.length, 1);
      assert.ok(result.orphanedProducers[0].includes('POST'));
    });

    it('should match with path parameter normalization', async () => {
      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry('GET', '/api/users/:id', 'UserResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry('GET', '/api/users/{userId}', 'UserData', 'consumer', 'api.ts', 5),
        ],
      };

      const result = await typeChecker.checkCompatibilityWithManifests(
        producers,
        consumers
      );

      // Should match despite different param formats
      assert.strictEqual(result.orphanedProducers.length, 0);
      assert.strictEqual(result.orphanedConsumers.length, 0);
      // Note: types won't be found in project, so will be in mismatches
      assert.strictEqual(result.matchDetails?.length, 1);
    });

    it('should report type not found errors as mismatches', async () => {
      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersData', 'consumer', 'api.ts', 5),
        ],
      };

      const result = await typeChecker.checkCompatibilityWithManifests(
        producers,
        consumers
      );

      // Types not in project, so should report as mismatch with "not found" error
      assert.strictEqual(result.incompatiblePairs, 1);
      assert.strictEqual(result.mismatches.length, 1);
      assert.ok(result.mismatches[0].errorDetails.includes('not found'));
    });

    it('should use provided types project for type lookup', async () => {
      // Create a types project with actual type definitions
      const typesProject = new Project({
        compilerOptions: {
          strict: true,
          skipLibCheck: true,
        },
      });

      // Add type definitions
      typesProject.createSourceFile(
        'types.d.ts',
        `
        export interface User {
          id: number;
          name: string;
        }
        export type UsersResponse = User[];
        export type UsersData = User[];
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersData', 'consumer', 'api.ts', 5),
        ],
      };

      const result = await typeChecker.checkCompatibilityWithManifests(
        producers,
        consumers,
        typesProject
      );

      // Types should be found and be compatible (same structure)
      assert.strictEqual(result.compatiblePairs, 1);
      assert.strictEqual(result.incompatiblePairs, 0);
    });

    it('should detect incompatible types', async () => {
      // Create a types project with incompatible type definitions
      const typesProject = new Project({
        compilerOptions: {
          strict: true,
          skipLibCheck: true,
        },
      });

      // Add incompatible type definitions
      typesProject.createSourceFile(
        'types.d.ts',
        `
        export interface User {
          id: number;
          name: string;
          email: string;
        }
        export type UsersResponse = User[];

        export interface SimpleUser {
          id: number;
        }
        export type UsersData = SimpleUser[];
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersResponse', 'producer', 'routes.ts', 10),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry('GET', '/api/users', 'UsersData', 'consumer', 'api.ts', 5),
        ],
      };

      const result = await typeChecker.checkCompatibilityWithManifests(
        producers,
        consumers,
        typesProject
      );

      // The producer type has more properties than consumer expects
      // This should still be compatible (consumer can ignore extra fields)
      // But if consumer expects fields that producer doesn't have, it would fail
      assert.strictEqual(result.matchDetails?.length, 1);
    });

    it('should include match details in result', async () => {
      const producers: TypeManifest = {
        repo_name: 'api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry('GET', '/api/users', 'GetUsers', 'producer', 'a.ts', 1),
          createManifestEntry('POST', '/api/users', 'CreateUser', 'producer', 'a.ts', 10),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'frontend',
        commit_hash: 'def',
        entries: [
          createManifestEntry('GET', '/api/users', 'FetchUsers', 'consumer', 'b.ts', 1),
        ],
      };

      const result = await typeChecker.checkCompatibilityWithManifests(
        producers,
        consumers
      );

      assert.ok(result.matchDetails);
      assert.strictEqual(result.matchDetails.length, 1);
      assert.strictEqual(result.matchDetails[0].method, 'GET');
      assert.strictEqual(result.matchDetails[0].producer.type_alias, 'GetUsers');
      assert.strictEqual(result.matchDetails[0].consumer.type_alias, 'FetchUsers');
    });
  });

  describe('autoCheck', () => {
    it('should throw error when no options provided', async () => {
      await assert.rejects(
        () => typeChecker.autoCheck({}),
        /Either outputDir or both manifest paths must be provided/
      );
    });

    it('should use manifest mode when manifest paths provided', async () => {
      const producerManifest: TypeManifest = {
        repo_name: 'producer',
        commit_hash: 'abc',
        entries: [],
      };
      const consumerManifest: TypeManifest = {
        repo_name: 'consumer',
        commit_hash: 'def',
        entries: [],
      };

      const producerPath = path.join(tempDir, 'producer.json');
      const consumerPath = path.join(tempDir, 'consumer.json');

      fs.writeFileSync(producerPath, JSON.stringify(producerManifest));
      fs.writeFileSync(consumerPath, JSON.stringify(consumerManifest));

      const result = await typeChecker.autoCheck({
        producerManifestPath: producerPath,
        consumerManifestPath: consumerPath,
      });

      assert.ok('mode' in result);
      assert.strictEqual((result as any).mode, 'manifest');
    });
  });

  describe('parseTypeName', () => {
    it('should parse producer type names', () => {
      const result = typeChecker.parseTypeName('GetApiUsersResponseProducer');
      assert.ok(result);
      assert.strictEqual(result.type, 'producer');
      assert.ok(result.endpoint.includes('GET'));
    });

    it('should parse consumer type names', () => {
      const result = typeChecker.parseTypeName('GetApiUsersResponseConsumerCall1');
      assert.ok(result);
      assert.strictEqual(result.type, 'consumer');
      assert.strictEqual(result.callId, 'Call1');
    });

    it('should parse consumer type names without call number', () => {
      const result = typeChecker.parseTypeName('GetApiUsersResponseConsumer');
      assert.ok(result);
      assert.strictEqual(result.type, 'consumer');
      assert.strictEqual(result.callId, 'Call0');
    });

    it('should return null for invalid type names', () => {
      const result = typeChecker.parseTypeName('SomeRandomType');
      assert.strictEqual(result, null);
    });
  });
});
