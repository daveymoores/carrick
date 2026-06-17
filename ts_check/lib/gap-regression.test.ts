/**
 * Gap-regression tests for manifest matching and the compatibility verdict.
 *
 * These confirm (and then guard the fixes for) two classes of bugs:
 *
 * 1. False-compatible verdicts from the type checker: the old probe
 *    re-serialized both types into a temp file in a project where the
 *    types' import references could not resolve. Any non-assignability
 *    diagnostic (Cannot find module, syntax) was filtered out, so broken
 *    or drifted types were reported compatible. Aliases that degrade to
 *    `any` (broken imports in bundled .d.ts) were silently compatible.
 *
 * 2. Over-eager path matching: a consumer of `/users/me` matched a
 *    `/users/:id` producer even when a literal `/users/me` producer
 *    existed, producing duplicate or contradictory verdicts.
 */

import { describe, it, beforeEach } from 'node:test';
import assert from 'node:assert';
import { Project } from 'ts-morph';
import { TypeCompatibilityChecker } from './type-checker';
import {
  TypeManifest,
  createManifestEntry,
  ManifestMatcher,
} from './manifest-matcher';

function makeTypesProject(dts: string): Project {
  const project = new Project({
    compilerOptions: { strict: true, skipLibCheck: true },
  });
  project.createSourceFile('types.d.ts', dts, { overwrite: true });
  return project;
}

function singleEndpointManifests(): {
  producers: TypeManifest;
  consumers: TypeManifest;
} {
  return {
    producers: {
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
    },
    consumers: {
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
    },
  };
}

describe('Compatibility verdict gap regressions', () => {
  let typeChecker: TypeCompatibilityChecker;

  beforeEach(() => {
    typeChecker = new TypeCompatibilityChecker(
      new Project({ compilerOptions: { strict: true, skipLibCheck: true } })
    );
  });

  it('detects incompatible types resolved from a types project', async () => {
    // Producer sends `id: string`, consumer requires `id: number`.
    // The old temp-file probe could not resolve the types' import
    // references and reported this pair compatible.
    const typesProject = makeTypesProject(`
      export type UsersResponse = { id: string };
      export type UsersData = { id: number };
    `);
    const { producers, consumers } = singleEndpointManifests();

    const result = await typeChecker.checkCompatibility(
      producers,
      consumers,
      typesProject
    );

    assert.strictEqual(result.compatiblePairs, 0);
    assert.strictEqual(result.incompatiblePairs, 1);
    assert.strictEqual(result.mismatches.length, 1);
  });

  it('treats aliases that degrade to any as unverifiable, not compatible', async () => {
    // A broken import in a bundled .d.ts degrades the alias to `any`,
    // which is assignable to anything — silently masking drift.
    const typesProject = makeTypesProject(`
      export type UsersResponse = import('./missing-module').Mystery;
      export type UsersData = { id: number };
    `);
    const { producers, consumers } = singleEndpointManifests();

    const result = await typeChecker.checkCompatibility(
      producers,
      consumers,
      typesProject
    );

    assert.strictEqual(
      result.compatiblePairs,
      0,
      'an alias that resolves to any must not be reported compatible'
    );
    assert.strictEqual(result.unknownPairs.length, 1);
    assert.ok(
      /any/.test(result.unknownPairs[0].reason),
      `expected reason to mention any, got: ${result.unknownPairs[0].reason}`
    );
  });

  it('compares wide anonymous types without truncation artifacts', async () => {
    // Wide payloads exceed the compiler's default ~160-char display
    // truncation. The mismatch must still be detected and reported with
    // the full type text (no `...`).
    const fields = Array.from(
      { length: 12 },
      (_, i) => `field_number_${i}: string;`
    ).join(' ');
    const typesProject = makeTypesProject(`
      export type UsersResponse = { ${fields} };
      export type UsersData = { ${fields} id: number; };
    `);
    const { producers, consumers } = singleEndpointManifests();

    const result = await typeChecker.checkCompatibility(
      producers,
      consumers,
      typesProject
    );

    assert.strictEqual(result.incompatiblePairs, 1);
    const mismatch = result.mismatches[0];
    assert.ok(
      !mismatch.producerType.includes('...'),
      `producer type text must not be truncated: ${mismatch.producerType}`
    );
    assert.ok(
      mismatch.producerType.includes('field_number_11'),
      `expected full type text, got: ${mismatch.producerType}`
    );
  });

  it('still reports structurally identical types as compatible', async () => {
    const typesProject = makeTypesProject(`
      export interface User { id: number; name: string; }
      export type UsersResponse = User[];
      export type UsersData = User[];
    `);
    const { producers, consumers } = singleEndpointManifests();

    const result = await typeChecker.checkCompatibility(
      producers,
      consumers,
      typesProject
    );

    assert.strictEqual(result.compatiblePairs, 1);
    assert.strictEqual(result.incompatiblePairs, 0);
    assert.strictEqual(result.unknownPairs.length, 0);
  });
});

describe('Most-specific path matching gap regressions', () => {
  const matcher = new ManifestMatcher();

  function producerManifest(paths: string[]): TypeManifest {
    return {
      repo_name: 'producer-api',
      commit_hash: 'abc',
      entries: paths.map((p, i) =>
        createManifestEntry('GET', p, `Producer${i}`, 'producer', `routes${i}.ts`, 10 + i)
      ),
    };
  }

  function consumerManifest(paths: string[]): TypeManifest {
    return {
      repo_name: 'consumer-app',
      commit_hash: 'def',
      entries: paths.map((p, i) =>
        createManifestEntry('GET', p, `Consumer${i}`, 'consumer', `api${i}.ts`, 5 + i)
      ),
    };
  }

  it('prefers a literal producer over a param producer', () => {
    // Express-style routing semantics: the literal route wins. Matching
    // the consumer against BOTH producers yields duplicate verdicts and
    // can flag drift against an endpoint the call never reaches.
    const producers = producerManifest(['/users/me', '/users/:id']);
    const consumers = consumerManifest(['/users/me']);

    const { matches, orphanedProducers } = matcher.matchEndpoints(
      producers,
      consumers
    );

    assert.strictEqual(matches.length, 1, 'expected exactly one match');
    assert.strictEqual(matches[0].producer.path, '/users/me');
    assert.strictEqual(orphanedProducers.length, 1);
    assert.strictEqual(orphanedProducers[0].entry.path, '/users/:id');
  });

  it('still matches a param producer when no literal producer exists', () => {
    const producers = producerManifest(['/users/:id']);
    const consumers = consumerManifest(['/users/42']);

    const { matches, orphanedConsumers } = matcher.matchEndpoints(
      producers,
      consumers
    );

    assert.strictEqual(matches.length, 1);
    assert.strictEqual(matches[0].producer.path, '/users/:id');
    assert.strictEqual(orphanedConsumers.length, 0);
  });

  it('keeps all equally specific producers', () => {
    // Duplicate registrations of the same route (e.g. two service
    // versions) are equally specific; both verdicts are wanted.
    const producers = producerManifest(['/users/:id', '/users/:userId']);
    const consumers = consumerManifest(['/users/7']);

    const { matches } = matcher.matchEndpoints(producers, consumers);

    assert.strictEqual(matches.length, 2);
  });
});
