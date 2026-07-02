/**
 * Tests for TypeCompatibilityChecker
 *
 * Tests cover:
 * - Manifest loading and parsing
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
import { TypeCompatibilityChecker } from './type-checker';
import { TypeManifest, ManifestEntry, createManifestEntry } from './manifest-matcher';

/**
 * Build a socket `payment:settled` SERVER->CLIENT manifest entry. Socket entries
 * carry `protocol: 'socket'` + `event` + `direction` instead of HTTP method/path.
 */
function socketEntry(
  typeAlias: string,
  role: 'producer' | 'consumer',
  filePath: string,
  lineNumber: number
): ManifestEntry {
  return {
    protocol: 'socket',
    event: 'payment:settled',
    direction: 'server_to_client',
    type_alias: typeAlias,
    role,
    type_kind: 'response',
    file_path: filePath,
    line_number: lineNumber,
    is_explicit: true,
    type_state: 'explicit',
    evidence: {
      file_path: filePath,
      span_start: null,
      span_end: null,
      line_number: lineNumber,
      infer_kind: 'response_body',
      is_explicit: true,
      type_state: 'explicit',
    },
  };
}

/**
 * Build a graphql `<kind> <field>` manifest entry. GraphQL entries carry
 * `protocol: 'graphql'` + `kind` + `field` instead of HTTP method/path.
 */
function graphqlEntry(
  typeAlias: string,
  role: 'producer' | 'consumer',
  filePath: string,
  lineNumber: number,
  kind: string = 'query',
  field: string = 'order'
): ManifestEntry {
  return {
    protocol: 'graphql',
    kind,
    field,
    type_alias: typeAlias,
    role,
    type_kind: 'response',
    file_path: filePath,
    line_number: lineNumber,
    is_explicit: true,
    type_state: 'explicit',
    evidence: {
      file_path: filePath,
      span_start: null,
      span_end: null,
      line_number: lineNumber,
      infer_kind: 'response_body',
      is_explicit: true,
      type_state: 'explicit',
    },
  };
}

/**
 * Build a pub/sub `order.placed` manifest entry. Pub/sub entries carry
 * `protocol: 'pubsub'` + `topic` instead of HTTP method/path (the broker is NOT
 * part of identity). The subscriber is keyed as the producer and the publisher
 * as the consumer, so pub/sub shares the socket inverted direction.
 */
function pubsubEntry(
  typeAlias: string,
  role: 'producer' | 'consumer',
  filePath: string,
  lineNumber: number,
  topic: string = 'order.placed'
): ManifestEntry {
  return {
    protocol: 'pubsub',
    topic,
    type_alias: typeAlias,
    role,
    type_kind: 'response',
    file_path: filePath,
    line_number: lineNumber,
    is_explicit: true,
    type_state: 'explicit',
    evidence: {
      file_path: filePath,
      span_start: null,
      span_end: null,
      line_number: lineNumber,
      infer_kind: 'response_body',
      is_explicit: true,
      type_state: 'explicit',
    },
  };
}

// ============================================================================
// TypeCompatibilityChecker Tests
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

  // --------------------------------------------------------------------------
  // Manifest Loading Tests
  // --------------------------------------------------------------------------

  describe('loadManifest', () => {
    it('should load a valid manifest file', () => {
      const manifest: TypeManifest = {
        repo_name: 'test-api',
        commit_hash: 'abc123def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'GetUsersResponse',
            'producer',
            'routes.ts',
            10
          ),
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

  describe('createEmptyManifest', () => {
    it('should create an empty manifest with correct fields', () => {
      const manifest = typeChecker.createEmptyManifest('my-repo', 'sha256hash');

      assert.strictEqual(manifest.repo_name, 'my-repo');
      assert.strictEqual(manifest.commit_hash, 'sha256hash');
      assert.deepStrictEqual(manifest.entries, []);
    });
  });

  describe('getMatcher', () => {
    it('should return the ManifestMatcher instance', () => {
      const matcher = typeChecker.getMatcher();
      assert.ok(matcher);
      assert.strictEqual(typeof matcher.loadManifest, 'function');
    });
  });

  // --------------------------------------------------------------------------
  // checkCompatibility Tests
  // --------------------------------------------------------------------------

  describe('checkCompatibility', () => {
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

      const result = await typeChecker.checkCompatibility(producers, consumers);

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
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersResponse',
            'producer',
            'routes.ts',
            10
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersData',
            'consumer',
            'api.ts',
            5
          ),
          createManifestEntry(
            'DELETE',
            '/api/users/:id',
            'DeleteResult',
            'consumer',
            'api.ts',
            15
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(producers, consumers);

      assert.strictEqual(result.orphanedConsumers.length, 1);
      assert.ok(result.orphanedConsumers[0].includes('DELETE'));
    });

    it('should identify orphaned producers', async () => {
      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersResponse',
            'producer',
            'routes.ts',
            10
          ),
          createManifestEntry(
            'POST',
            '/api/users',
            'CreateResponse',
            'producer',
            'routes.ts',
            20
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersData',
            'consumer',
            'api.ts',
            5
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(producers, consumers);

      assert.strictEqual(result.orphanedProducers.length, 1);
      assert.ok(result.orphanedProducers[0].includes('POST'));
    });

    it('should report unknown type pairs with evidence', async () => {
      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersResponse',
            'producer',
            'routes.ts',
            10,
            'response',
            false,
            'unknown'
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersData',
            'consumer',
            'api.ts',
            5,
            'response',
            false,
            'unknown'
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(producers, consumers);

      assert.strictEqual(result.unknownPairs.length, 1);
      assert.strictEqual(result.mismatches.length, 0);
      assert.strictEqual(result.compatiblePairs, 0);
      assert.strictEqual(result.incompatiblePairs, 0);
      assert.strictEqual(result.unknownPairs[0].producerEvidence?.file_path, 'routes.ts');
      assert.strictEqual(result.unknownPairs[0].consumerEvidence?.file_path, 'api.ts');
    });

    it('should compare types when type_state is unknown but aliases resolve', async () => {
      const typesProject = new Project({
        compilerOptions: {
          strict: true,
          skipLibCheck: true,
        },
      });

      typesProject.createSourceFile(
        'types.d.ts',
        `
        export type UsersResponse = { id: number };
        export type UsersData = { id: number };
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersResponse',
            'producer',
            'routes.ts',
            10,
            'response',
            false,
            'unknown'
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersData',
            'consumer',
            'api.ts',
            5,
            'response',
            false,
            'unknown'
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.compatiblePairs, 1);
      assert.strictEqual(result.incompatiblePairs, 0);
      assert.strictEqual(result.unknownPairs.length, 0);
    });

    it('should treat an unknown-resolving side as unverifiable, not compatible', async () => {
      // Regression for #235: when the consumer alias is missing from the bundle
      // it is injected as `= unknown` by append_missing_aliases. Everything is
      // assignable to `unknown`, so without an isUnknown() guard this edge read
      // compatible and masked a real shape mismatch. It must be unverifiable.
      const typesProject = new Project({
        compilerOptions: {
          strict: true,
          skipLibCheck: true,
        },
      });

      typesProject.createSourceFile(
        'types.d.ts',
        `
        export type Order = { id: number; amountCents: number; currency: string };
        export type OrderView = unknown;
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'orders-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry(
            'GET',
            '/orders/:id',
            'Order',
            'producer',
            'routes.ts',
            10
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/orders/:id',
            'OrderView',
            'consumer',
            'api.ts',
            5
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.unknownPairs.length, 1);
      assert.strictEqual(result.compatiblePairs, 0);
      assert.strictEqual(result.incompatiblePairs, 0);
      assert.ok(result.unknownPairs[0].reason.includes('unknown'));
    });

    it('should report a string-vs-number field mismatch as incompatible', async () => {
      // The genuine orders→web defect (#235): producer Order.id is number,
      // consumer OrderView.id is string. With the real shapes in the bundle the
      // edge must read incompatible.
      const typesProject = new Project({
        compilerOptions: {
          strict: true,
          skipLibCheck: true,
        },
      });

      typesProject.createSourceFile(
        'types.d.ts',
        `
        export type Order = { id: number; amountCents: number; currency: string };
        export type OrderView = { id: string; currency: string };
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'orders-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry(
            'GET',
            '/orders/:id',
            'Order',
            'producer',
            'routes.ts',
            10
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/orders/:id',
            'OrderView',
            'consumer',
            'api.ts',
            5
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.incompatiblePairs, 1);
      assert.strictEqual(result.compatiblePairs, 0);
      assert.strictEqual(result.unknownPairs.length, 0);
      assert.strictEqual(result.mismatches.length, 1);
    });

    it('type-checks a socket SERVER->CLIENT edge end-to-end as compatible', async () => {
      // The xrepo-corpus-1 `payment:settled` edge. Carrick keys the *listener*
      // (web-frontend, `SettledPayment`) as the producer and the *emitter*
      // (payments-svc, `Payment`) as the consumer. The bytes flow emitter →
      // listener, so the emitter payload must satisfy what the listener expects:
      // `Payment.status: "pending" | "settled"` ⊑ `SettledPayment.status: string`
      // → compatible. (Reusing the HTTP direction would wrongly read this as
      // incompatible.)
      const typesProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      typesProject.createSourceFile(
        'types.d.ts',
        `
        export type SettledPayment = { id: string; orderId: number; amountCents: number; status: string };
        export type Payment = { id: string; orderId: number; amountCents: number; status: "pending" | "settled" };
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'abc',
        entries: [
          socketEntry('SettledPayment', 'producer', 'lib/realtime.ts', 31),
        ],
      };
      const consumers: TypeManifest = {
        repo_name: 'payments-svc',
        commit_hash: 'def',
        entries: [
          socketEntry('Payment', 'consumer', 'realtime/server.ts', 27),
        ],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.matchDetails?.length, 1, 'the socket pair must match');
      assert.strictEqual(
        result.matchDetails?.[0].method,
        'SOCKET',
        'the match is keyed on the SOCKET pseudo-method'
      );
      assert.strictEqual(
        result.matchDetails?.[0].path,
        'SERVER->CLIENT|payment:settled',
        'the match path is the canonical socket key tail'
      );
      assert.strictEqual(result.compatiblePairs, 1);
      assert.strictEqual(result.incompatiblePairs, 0);
      assert.strictEqual(result.unknownPairs.length, 0);
    });

    it('reads a socket edge whose emitted payload widens the listener type as incompatible', async () => {
      // Direction proof: if the emitter sends a *wider* type than the listener
      // accepts, the edge is incompatible. Here the emitter sends
      // `status: string` but the listener only accepts `"pending" | "settled"`,
      // so `string` is NOT assignable → incompatible. This is the mirror of the
      // compatible case and fails if the assignability direction is not flipped
      // for sockets.
      const typesProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      typesProject.createSourceFile(
        'types.d.ts',
        `
        export type StrictPayment = { id: string; status: "pending" | "settled" };
        export type LoosePayment = { id: string; status: string };
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'listener-repo',
        commit_hash: 'abc',
        entries: [socketEntry('StrictPayment', 'producer', 'lib/realtime.ts', 31)],
      };
      const consumers: TypeManifest = {
        repo_name: 'emitter-repo',
        commit_hash: 'def',
        entries: [socketEntry('LoosePayment', 'consumer', 'realtime/server.ts', 27)],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.incompatiblePairs, 1);
      assert.strictEqual(result.compatiblePairs, 0);
      assert.strictEqual(result.mismatches.length, 1);
      assert.ok(
        result.mismatches[0].endpoint.startsWith('SOCKET '),
        'the mismatch endpoint must carry the SOCKET label'
      );
    });

    it('type-checks a graphql query|order edge end-to-end as compatible (consumer selects a subset of the producer payload)', async () => {
      // The xrepo-corpus-1 `graphql|query|order` edge. The producer (gateway
      // resolver) resolves to the return ENVELOPE `{ data: Order; errors }`; the
      // SDL field payload is `Order`. The consumer document binds `OrderView`,
      // which SELECTS a subset of the producer fields (`id`, `total`, optional
      // `note`) and DROPS the producer's required `status` field — valid GraphQL
      // field selection, so the edge is COMPATIBLE.
      //
      // PRE-FIX-FAILING: before the graphql arm in compareTypes, a graphql
      // producer was never compared (entries were dropped), so this pair could
      // not be checked at all. With a naive socket-style direction
      // (`consumer ⊑ producer`) it would read INCOMPATIBLE (OrderView lacks the
      // producer's required `status`). It only reads compatible once the producer
      // envelope is structurally unwrapped to `Order` AND the data-flow direction
      // `producer ⊑ consumer` (HTTP-like) is used.
      const typesProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      typesProject.createSourceFile(
        'types.d.ts',
        `
        export type Money = { amountCents: number; currency: string };
        export type OrderStatus =
          | { kind: "placed"; placedAt: string }
          | { kind: "refunded"; refundedAt: string; reason?: string };
        // Producer resolved type: the resolver RETURN ENVELOPE, not the payload.
        export type ProducerEnvelope = {
          data: { id: string; total: Money; status: OrderStatus; note?: string };
          errors: string[];
        };
        export type MoneyView = { amountCents: number; currency: string };
        // Consumer bound type: selects a subset, drops \`status\`.
        export type OrderView = { id: string; total: MoneyView; note?: string };
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'orders-monorepo',
        commit_hash: 'abc',
        entries: [
          graphqlEntry('ProducerEnvelope', 'producer', 'gateway/src/orders.resolver.ts', 40),
        ],
      };
      const consumers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'def',
        entries: [
          graphqlEntry('OrderView', 'consumer', 'lib/graphql.ts', 76),
        ],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.matchDetails?.length, 1, 'the graphql pair must match');
      assert.strictEqual(
        result.matchDetails?.[0].method,
        'GRAPHQL',
        'the match is keyed on the GRAPHQL pseudo-method'
      );
      assert.strictEqual(
        result.matchDetails?.[0].path,
        'query|order',
        'the match path is the canonical graphql key tail'
      );
      assert.strictEqual(result.compatiblePairs, 1);
      assert.strictEqual(result.incompatiblePairs, 0);
      assert.strictEqual(result.unknownPairs.length, 0);
    });

    it('reads a graphql edge whose consumer requires a field the producer makes optional as incompatible (direction proof)', async () => {
      // Direction/anchor proof, mirroring the corpus `subscription orderUpdated`
      // trap: the producer payload makes `note` OPTIONAL, but the consumer makes
      // it REQUIRED. Under `producer ⊑ consumer`, the producer payload
      // `{ id; note?: string }` is NOT assignable to the consumer
      // `{ id; note: string }` (optional can't satisfy required) → INCOMPATIBLE.
      // This fails if the envelope is not unwrapped (the whole envelope is never
      // assignable, but for the wrong reason) or if the direction is flipped
      // (the consumer requiring more than the producer guarantees would wrongly
      // read compatible).
      const typesProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      typesProject.createSourceFile(
        'types.d.ts',
        `
        export type ProducerEnvelope = {
          data: { id: string; note?: string };
          errors: string[];
        };
        // Consumer requires \`note\`; producer only optionally provides it.
        export type OrderUpdate = { id: string; note: string };
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'orders-monorepo',
        commit_hash: 'abc',
        entries: [
          graphqlEntry('ProducerEnvelope', 'producer', 'gateway/src/orders.resolver.ts', 75, 'subscription', 'orderUpdated'),
        ],
      };
      const consumers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'def',
        entries: [
          graphqlEntry('OrderUpdate', 'consumer', 'lib/graphql.ts', 80, 'subscription', 'orderUpdated'),
        ],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.incompatiblePairs, 1);
      assert.strictEqual(result.compatiblePairs, 0);
      assert.strictEqual(result.unknownPairs.length, 0);
      assert.strictEqual(result.mismatches.length, 1);
      assert.ok(
        result.mismatches[0].endpoint.startsWith('GRAPHQL '),
        'the mismatch endpoint must carry the GRAPHQL label'
      );
    });

    it('reads a graphql edge whose consumer resolves to `any` as unverifiable, NOT compatible (subscription-orderUpdated false-positive)', async () => {
      // The live `graphql|subscription|orderUpdated` false-positive: the PRODUCER
      // resolves to a real `Order` envelope (Explicit), but the CONSUMER's
      // synthetic dangling alias resolves to `any` in the bundle. The graphql
      // direction is `producer ⊑ consumer`, so `Order ⊑ any` is trivially TRUE —
      // the edge would read COMPATIBLE without the any/unknown guard, masking that
      // the consumer type never reached the bundle. It MUST read unverifiable.
      const typesProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      typesProject.createSourceFile(
        'types.d.ts',
        `
        // Producer: real resolver return envelope unwrapping to a concrete Order.
        export type ProducerEnvelope = {
          data: { id: string; total: number; note?: string };
          errors: string[];
        };
        // Consumer: synthetic alias referencing a type with no declaration in the
        // bundle → resolves to \`any\` (the dangling-anchor shape).
        export type OrderUpdateView = MissingShape;
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'orders-monorepo',
        commit_hash: 'abc',
        entries: [
          graphqlEntry('ProducerEnvelope', 'producer', 'gateway/src/orders.resolver.ts', 75, 'subscription', 'orderUpdated'),
        ],
      };
      const consumers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'def',
        entries: [
          graphqlEntry('OrderUpdateView', 'consumer', 'lib/graphql.ts', 80, 'subscription', 'orderUpdated'),
        ],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.compatiblePairs, 0, 'must NOT read compatible');
      assert.strictEqual(result.incompatiblePairs, 0);
      assert.strictEqual(
        result.unknownPairs.length,
        1,
        'a graphql consumer resolving to `any` must read unverifiable'
      );
      assert.ok(
        result.unknownPairs[0].reason.includes('any'),
        `the unverifiable reason must name the resolved \`any\`, got: ${result.unknownPairs[0].reason}`
      );
    });

    it('reads a graphql edge whose consumer resolves to `unknown` as unverifiable, NOT compatible', async () => {
      // The `unknown` twin of the subscription-orderUpdated trap: `Order ⊑ unknown`
      // is also trivially TRUE, so an unguarded edge would read compatible.
      const typesProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      typesProject.createSourceFile(
        'types.d.ts',
        `
        export type ProducerEnvelope = {
          data: { id: string; total: number };
          errors: string[];
        };
        export type OrderUpdateView = unknown;
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'orders-monorepo',
        commit_hash: 'abc',
        entries: [
          graphqlEntry('ProducerEnvelope', 'producer', 'gateway/src/orders.resolver.ts', 75, 'subscription', 'orderUpdated'),
        ],
      };
      const consumers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'def',
        entries: [
          graphqlEntry('OrderUpdateView', 'consumer', 'lib/graphql.ts', 80, 'subscription', 'orderUpdated'),
        ],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.compatiblePairs, 0, 'must NOT read compatible');
      assert.strictEqual(result.incompatiblePairs, 0);
      assert.strictEqual(result.unknownPairs.length, 1);
    });

    it('type-checks a pub/sub edge end-to-end as compatible (subscriber accepts a wider payload than the publisher sends)', async () => {
      // The corpus-2 `order.placed` edge. Carrick keys the *subscriber*
      // (orders-svc handler, `WideOrder`) as the producer and the *publisher*
      // (checkout-svc send, `StrictOrder`) as the consumer. The bytes flow
      // publisher → subscriber, so the publisher payload must satisfy what the
      // subscriber accepts: `StrictOrder.status: "placed" | "paid"` ⊑
      // `WideOrder.status: string` → compatible. (Reusing the HTTP direction
      // would wrongly read this as incompatible; this is the socket inversion.)
      const typesProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      typesProject.createSourceFile(
        'types.d.ts',
        `
        export type WideOrder = { id: string; totalCents: number; status: string };
        export type StrictOrder = { id: string; totalCents: number; status: "placed" | "paid" };
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'orders-svc',
        commit_hash: 'abc',
        entries: [pubsubEntry('WideOrder', 'producer', 'src/consumer.ts', 14)],
      };
      const consumers: TypeManifest = {
        repo_name: 'checkout-svc',
        commit_hash: 'def',
        entries: [pubsubEntry('StrictOrder', 'consumer', 'src/publisher.ts', 22)],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.matchDetails?.length, 1, 'the pub/sub pair must match');
      assert.strictEqual(
        result.matchDetails?.[0].method,
        'PUBSUB',
        'the match is keyed on the PUBSUB pseudo-method'
      );
      assert.strictEqual(
        result.matchDetails?.[0].path,
        'order.placed',
        'the match path is the bare topic'
      );
      assert.strictEqual(result.compatiblePairs, 1);
      assert.strictEqual(result.incompatiblePairs, 0);
      assert.strictEqual(result.unknownPairs.length, 0);
    });

    it('reads a pub/sub edge whose published payload widens the subscriber type as incompatible (inverted-direction proof)', async () => {
      // Direction proof (mirrors the socket widening test): if the publisher
      // sends a *wider* type than the subscriber accepts, the edge is
      // incompatible. Here the publisher sends `status: string` but the
      // subscriber only accepts `"placed" | "paid"`, so `string` is NOT
      // assignable → incompatible. This is the incompatible Kafka `order.placed`
      // edge, and it fails if the assignability direction is not flipped for
      // pub/sub (the same flip as socket).
      const typesProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      typesProject.createSourceFile(
        'types.d.ts',
        `
        export type StrictOrder = { id: string; status: "placed" | "paid" };
        export type WideOrder = { id: string; status: string };
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'subscriber-repo',
        commit_hash: 'abc',
        entries: [pubsubEntry('StrictOrder', 'producer', 'src/consumer.ts', 14)],
      };
      const consumers: TypeManifest = {
        repo_name: 'publisher-repo',
        commit_hash: 'def',
        entries: [pubsubEntry('WideOrder', 'consumer', 'src/publisher.ts', 22)],
      };

      const result = await typeChecker.checkCompatibility(
        producers,
        consumers,
        typesProject
      );

      assert.strictEqual(result.incompatiblePairs, 1);
      assert.strictEqual(result.compatiblePairs, 0);
      assert.strictEqual(result.mismatches.length, 1);
      assert.ok(
        result.mismatches[0].endpoint.startsWith('PUBSUB '),
        'the mismatch endpoint must carry the PUBSUB label'
      );
    });

    it('reads a dangling consumer name as unverifiable but its structural shape as incompatible (#257)', async () => {
      // The #257 inference bug: a consumer like `res.json() as Promise<OrderView>`
      // wrote the bare NAME `= OrderView` into the cross-repo bundle. The bundle
      // carries only alias lines and no source declaration for `OrderView`, so
      // the name dangles → resolves to `any` → the edge reads unverifiable and
      // the genuine string-vs-number mismatch is masked.
      //
      // Producer `Order.id` is number; consumer `OrderView.id` is string. The two
      // halves of this test pin both ends of the fix:
      //  - bundling the bare dangling name reproduces the masked `unverifiable`;
      //  - bundling the STRUCTURAL shape the fixed inferrer now emits surfaces the
      //    real `incompatible`.
      const danglingProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      // `= MissingShape` referencing a type with no declaration in the bundle —
      // exactly the dangling alias the old inference path produced. It resolves
      // to `any`, so the comparison is unverifiable.
      danglingProject.createSourceFile(
        'types.d.ts',
        `
        export type Order = { id: number; amountCents: number; currency: string };
        export type OrderView = MissingShape;
        `,
        { overwrite: true }
      );

      const structuralProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      // The member shape the fixed inferrer now emits for the same consumer.
      structuralProject.createSourceFile(
        'types.d.ts',
        `
        export type Order = { id: number; amountCents: number; currency: string };
        export type OrderView = { id: string; currency: string };
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'orders-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry(
            'GET',
            '/orders/:id',
            'Order',
            'producer',
            'routes.ts',
            10
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/orders/:id',
            'OrderView',
            'consumer',
            'api.ts',
            5
          ),
        ],
      };

      const dangling = await typeChecker.checkCompatibility(
        producers,
        consumers,
        danglingProject
      );
      // Before the fix: the dangling name masks the mismatch as unverifiable.
      assert.strictEqual(dangling.incompatiblePairs, 0);
      assert.strictEqual(dangling.compatiblePairs, 0);
      assert.strictEqual(dangling.unknownPairs.length, 1);

      const structural = await typeChecker.checkCompatibility(
        producers,
        consumers,
        structuralProject
      );
      // After the fix: the real shape surfaces the genuine string-vs-number mismatch.
      assert.strictEqual(structural.incompatiblePairs, 1);
      assert.strictEqual(structural.compatiblePairs, 0);
      assert.strictEqual(structural.unknownPairs.length, 0);
      assert.strictEqual(structural.mismatches.length, 1);
    });

    it('flags only the Carrick-marked `= unknown` as the injected placeholder (#244)', async () => {
      // The marked alias is the injected placeholder: it short-circuits on the
      // type_state=unknown path. The unmarked, developer-authored `= unknown`
      // resolves to a genuine `unknown` and is caught by the compiler-level
      // isUnknown() gate instead. Both are unverifiable, but only the marked one
      // is treated as our placeholder, with the corresponding reason text.
      const markedProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      markedProject.createSourceFile(
        'types.d.ts',
        `
        export type Order = { id: number };
        export type OrderView = unknown; // carrick:missing-alias
        `,
        { overwrite: true }
      );

      const bareProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      bareProject.createSourceFile(
        'types.d.ts',
        `
        export type Order = { id: number };
        export type OrderView = unknown;
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'orders-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry('GET', '/orders/:id', 'Order', 'producer', 'routes.ts', 10),
        ],
      };
      // type_state='unknown' so resolveTypeInfo (and isPlaceholderUnknown) runs.
      const consumers: TypeManifest = {
        repo_name: 'web-frontend',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/orders/:id',
            'OrderView',
            'consumer',
            'api.ts',
            5,
            'response',
            false,
            'unknown'
          ),
        ],
      };

      const marked = await typeChecker.checkCompatibility(
        producers,
        consumers,
        markedProject
      );
      assert.strictEqual(marked.unknownPairs.length, 1);
      assert.strictEqual(marked.compatiblePairs, 0);
      assert.strictEqual(marked.incompatiblePairs, 0);
      // Marked placeholder takes the early type_state=unknown short-circuit.
      assert.ok(
        marked.unknownPairs[0].reason.includes('type_state=unknown'),
        `marked placeholder should report type_state=unknown, got: ${marked.unknownPairs[0].reason}`
      );

      const bareChecker = new TypeCompatibilityChecker(bareProject);
      const bare = await bareChecker.checkCompatibility(
        producers,
        consumers,
        bareProject
      );
      assert.strictEqual(bare.unknownPairs.length, 1);
      assert.strictEqual(bare.compatiblePairs, 0);
      assert.strictEqual(bare.incompatiblePairs, 0);
      // Developer-authored `= unknown` is NOT the placeholder: it falls through
      // to the compiler-level gate, whose reason names the resolved `unknown`.
      assert.ok(
        !bare.unknownPairs[0].reason.includes('type_state=unknown'),
        `developer-authored unknown must not be flagged as the placeholder, got: ${bare.unknownPairs[0].reason}`
      );
      assert.ok(
        bare.unknownPairs[0].reason.includes('unknown'),
        `developer-authored unknown should still be unverifiable, got: ${bare.unknownPairs[0].reason}`
      );
    });

    it('abstains (unverifiable) when a NESTED field dangles to any via a kept-by-name library type (version-conflict safety)', async () => {
      // The measured false-compatible: a library/version type (e.g. bson ObjectId,
      // a zod-inferred class across majors) is serialized BY NAME with no import,
      // so at check time the field dangles. Here producer/consumer both read
      // `{ token: Token; amount: number }` with `Token` undeclared. The top-level
      // any/unknown guard MISSES it (the object type is not itself any; only the
      // nested `token` field is), so `isAssignableTo` compares any-vs-any on
      // `token` and reads COMPATIBLE — masking a real wire mismatch. It must
      // instead read unverifiable: a dangling reference means the shape never
      // reached the bundle and the verdict cannot be trusted.
      const typesProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      typesProject.createSourceFile(
        'types.d.ts',
        `
        // \`Token\` is a library class kept by name with no declaration in the
        // bundle (imports are stripped in the flat cross-repo project).
        export interface Endpoint_vc_Response { token: Token; amount: number; }
        export interface Endpoint_vc_Response_Call1 { token: Token; amount: number; }
        `,
        { overwrite: true }
      );

      const producers: TypeManifest = {
        repo_name: 'producer-svc', commit_hash: 'p',
        entries: [createManifestEntry('POST', '/x', 'Endpoint_vc_Response', 'producer', 'p.ts', 1)],
      };
      const consumers: TypeManifest = {
        repo_name: 'consumer-svc', commit_hash: 'c',
        entries: [createManifestEntry('POST', '/x', 'Endpoint_vc_Response_Call1', 'consumer', 'c.ts', 1)],
      };

      const result = await typeChecker.checkCompatibility(producers, consumers, typesProject);

      assert.strictEqual(result.compatiblePairs, 0, 'must NOT read compatible (would be a false-compatible)');
      assert.strictEqual(result.incompatiblePairs, 0);
      assert.strictEqual(result.unknownPairs.length, 1, 'a dangling nested reference must read unverifiable');
      assert.ok(
        /unresolved|dangl|not in the bundle|cannot verify/i.test(result.unknownPairs[0].reason),
        `the reason must name the dangling reference, got: ${result.unknownPairs[0].reason}`
      );
    });

    it('does NOT over-abstain: a cleanly-resolved pair with a genuine field type stays a definite verdict', async () => {
      // Guard against the fix over-firing: identical, cleanly-resolving inlined
      // shapes (e.g. a nominal class inlined to its wire shape) must keep their
      // definite verdict, not get swept into unverifiable. No dangling refs here.
      const typesProject = new Project({
        compilerOptions: { strict: true, skipLibCheck: true },
      });
      typesProject.createSourceFile(
        'types.d.ts',
        `
        export interface Endpoint_ok_Response { cents: number; toJSON: () => { cents: number } }
        export interface Endpoint_ok_Response_Call1 { cents: number; toJSON: () => { cents: number } }
        `,
        { overwrite: true }
      );
      const producers: TypeManifest = {
        repo_name: 'p', commit_hash: 'p',
        entries: [createManifestEntry('POST', '/x', 'Endpoint_ok_Response', 'producer', 'p.ts', 1)],
      };
      const consumers: TypeManifest = {
        repo_name: 'c', commit_hash: 'c',
        entries: [createManifestEntry('POST', '/x', 'Endpoint_ok_Response_Call1', 'consumer', 'c.ts', 1)],
      };
      const result = await typeChecker.checkCompatibility(producers, consumers, typesProject);
      assert.strictEqual(result.compatiblePairs, 1, 'cleanly-resolving identical shapes stay compatible');
      assert.strictEqual(result.unknownPairs.length, 0);
    });

    it('should match with path parameter normalization', async () => {
      const producers: TypeManifest = {
        repo_name: 'producer-api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users/:id',
            'UserResponse',
            'producer',
            'routes.ts',
            10
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users/{userId}',
            'UserData',
            'consumer',
            'api.ts',
            5
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(producers, consumers);

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
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersResponse',
            'producer',
            'routes.ts',
            10
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersData',
            'consumer',
            'api.ts',
            5
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(producers, consumers);

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
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersResponse',
            'producer',
            'routes.ts',
            10
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersData',
            'consumer',
            'api.ts',
            5
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(
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
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersResponse',
            'producer',
            'routes.ts',
            10
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'consumer-app',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersData',
            'consumer',
            'api.ts',
            5
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(
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
          createManifestEntry(
            'GET',
            '/api/users',
            'GetUsers',
            'producer',
            'a.ts',
            1
          ),
          createManifestEntry(
            'POST',
            '/api/users',
            'CreateUser',
            'producer',
            'a.ts',
            10
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'frontend',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'FetchUsers',
            'consumer',
            'b.ts',
            1
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(producers, consumers);

      assert.ok(result.matchDetails);
      assert.strictEqual(result.matchDetails.length, 1);
      assert.strictEqual(result.matchDetails[0].method, 'GET');
      assert.strictEqual(result.matchDetails[0].producer.type_alias, 'GetUsers');
      assert.strictEqual(
        result.matchDetails[0].consumer.type_alias,
        'FetchUsers'
      );
    });

    it('should handle multiple consumers for same endpoint', async () => {
      const producers: TypeManifest = {
        repo_name: 'api',
        commit_hash: 'abc',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'GetUsersResponse',
            'producer',
            'routes.ts',
            10
          ),
        ],
      };

      const consumers: TypeManifest = {
        repo_name: 'frontend',
        commit_hash: 'def',
        entries: [
          createManifestEntry(
            'GET',
            '/api/users',
            'UsersListData',
            'consumer',
            'users.ts',
            5
          ),
          createManifestEntry(
            'GET',
            '/api/users',
            'AdminUsersData',
            'consumer',
            'admin.ts',
            15
          ),
        ],
      };

      const result = await typeChecker.checkCompatibility(producers, consumers);

      // Both consumers should match the same producer
      assert.strictEqual(result.matchDetails?.length, 2);
      assert.ok(
        result.matchDetails?.every(
          (m) => m.producer.type_alias === 'GetUsersResponse'
        )
      );
      assert.strictEqual(result.orphanedProducers.length, 0);
      assert.strictEqual(result.orphanedConsumers.length, 0);
    });
  });
});
