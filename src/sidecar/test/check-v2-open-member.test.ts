/**
 * carrick#1398 slice A: a pair whose capture found an open type (`unknown`,
 * `any`) under a named member is compared on the rest of its type.
 *
 * - A mismatch on the typed members files `incompatible`, `resolved: false`,
 *   with the reason naming the open member and the diagnostic naming only the
 *   fields that differ.
 * - Anything else, an agreement included, reads exactly what the capture
 *   pre-gate wrote before: the comparison is one way.
 * - The sent side's `unknown` members are opened for the comparison, so an
 *   open member alone never files a mismatch.
 * - An exhausted budget, a top type at no named member and a GraphQL pair
 *   stay pre-gated, in today's words.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { captureStub, runCheck } from '../src/capture/index.js';
import { pathNamesAMember } from '../src/capture/check.js';
import { buildProbe } from '../src/capture/check-probe.js';
import type {
  CaptureStubResult,
  CheckPairSpec,
  CheckVerdict,
  TypeProvenance,
} from '../src/capture/api.js';

type Finding = Pick<TypeProvenance, 'path' | 'kind' | 'reason'>;

function captureLiterals(
  root: string,
  serviceName: string,
  literals: Record<string, string>
): CaptureStubResult {
  const repoRoot = path.join(root, `${serviceName}-repo`);
  fs.mkdirSync(repoRoot, { recursive: true });
  const result = captureStub({
    repoRoot,
    serviceName,
    outDir: path.join(root, `${serviceName}-stub`),
    anchors: Object.entries(literals).map(([alias, text]) => ({
      kind: 'literal' as const,
      alias,
      type_text: text,
      anchor_origin: 'deterministic-infer' as const,
    })),
  });
  assert.strictEqual(result.success, true, JSON.stringify(result.errors));
  return result;
}

interface ManifestFile {
  aliases: Array<{ alias: string; any_provenance?: Finding[] }>;
}

function readManifest(stub: CaptureStubResult): ManifestFile {
  return JSON.parse(
    fs.readFileSync(path.join(stub.stub_dir, 'carrick-manifest.json'), 'utf8')
  ) as ManifestFile;
}

/** Replace the findings the stub's manifest records for one alias. */
function setFindings(stub: CaptureStubResult, alias: string, findings: Finding[]): void {
  const manifest = readManifest(stub);
  const record = manifest.aliases.find((a) => a.alias === alias);
  assert.ok(record, `no manifest record for ${alias}`);
  record.any_provenance = findings;
  fs.writeFileSync(
    path.join(stub.stub_dir, 'carrick-manifest.json'),
    JSON.stringify(manifest, null, 2)
  );
}

function findingsOf(stub: CaptureStubResult, alias: string): Finding[] {
  return (readManifest(stub).aliases.find((a) => a.alias === alias)?.any_provenance ?? []).map(
    ({ path: p, kind, reason }) => ({ path: p, kind, reason })
  );
}

/** The verdict the capture pre-gate writes, word for word. */
function preGated(
  pairKey: string,
  side: 'producer' | 'consumer',
  kind: 'any' | 'unknown' | 'budget_exhausted',
  at: string
): Omit<CheckVerdict, 'pair_id'> {
  return {
    pair_key: pairKey,
    bucket: kind === 'any' ? 'gate_caught_baked_any' : 'unverifiable',
    gate: `capture:${side}:${kind}`,
    diagnostic:
      kind === 'budget_exhausted'
        ? `the ${side} type is too deep or wide to verify within the ` +
          `capture budget (at '${at}'); compatibility cannot be ` +
          `verified — abstaining so a buried 'any' can never read compatible.`
        : `the ${side} type carries '${kind}' at '${at}' from ` +
          `capture; compatibility cannot be verified (a partially-unresolved ` +
          `type would let an arbitrary shape read compatible).`,
    codes: [],
    resolved: false,
    unresolved_side: side,
    unresolved_reason:
      kind === 'budget_exhausted'
        ? `the ${side} type is too deep or wide to verify at '${at}'`
        : `the ${side} type carries '${kind}' at '${at}'`,
    notes: [],
  };
}

function withoutId(v: CheckVerdict): Omit<CheckVerdict, 'pair_id'> {
  const { pair_id: _id, ...rest } = v;
  return rest;
}

describe('check phase: a pair with an open member is compared on the rest (carrick#1398)', () => {
  let root: string;
  let producer: CaptureStubResult;
  let consumer: CaptureStubResult;
  let verdicts: Map<string, CheckVerdict>;

  const pair = (
    pair_key: string,
    producerAlias: string,
    consumerAlias: string,
    over: Partial<CheckPairSpec> = {}
  ): CheckPairSpec => ({
    pair_key,
    protocol: 'http',
    type_kind: 'response',
    producer: { service_name: 'om-producer', alias: producerAlias },
    consumer: { service_name: 'om-consumer', alias: consumerAlias },
    ...over,
  });
  const request = { type_kind: 'request' as const };

  before(async () => {
    root = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-1398-check-'));
    producer = captureLiterals(root, 'om-producer', {
      // 1. the consumer's member is open, the rest agrees
      P_Agree: '{ id: string; meta: { a: 1 } }',
      // 2. / 3. the producer's member is open
      P_UnknownMismatch: '{ id: number; raw: unknown }',
      P_UnknownAgree: '{ id: string; raw: unknown }',
      // 4. the same with `any`, recorded as an import that did not resolve
      P_AnyMismatch: '{ id: number; raw: any }',
      P_AnyAgree: '{ id: string; raw: any }',
      // a nested open member, in an element of a list
      P_NestedMismatch: '{ id: string; items: { n: number; ev: unknown }[] }',
      P_NestedAgree: '{ id: string; items: { n: string; ev: unknown }[] }',
      // 5. nothing to compare
      P_RootRecord: 'Record<string, unknown>',
      P_RootList: 'unknown[]',
      P_Budget: '{ id: number; deep: { x: string } }',
      // 6. request bodies: the producer expects
      P_ReqExpects: '{ id: string; raw: string }',
      P_ReqExpects2: '{ id: string; raw: string }',
      // a GraphQL resolver
      P_Gql: '{ id: number; raw: unknown }',
      // 7. no finding at all
      P_Plain: '{ id: number }',
      // the opened form loses a construct signature, which the declared one keeps
      P_Ctor: '{ make: new () => object; raw: unknown }',
    });
    consumer = captureLiterals(root, 'om-consumer', {
      C_Agree: '{ id: string; meta: unknown }',
      C_UnknownMismatch: '{ id: string; raw: string }',
      C_UnknownAgree: '{ id: string; raw: string }',
      C_AnyMismatch: '{ id: string; raw: string }',
      C_AnyAgree: '{ id: string; raw: string }',
      C_NestedMismatch: '{ id: string; items: { n: string; ev: string }[] }',
      C_NestedAgree: '{ id: string; items: { n: string; ev: string }[] }',
      C_RootRecord: '{ id: string }',
      C_RootList: '{ id: string }',
      C_Budget: '{ id: string; deep: { x: string } }',
      C_ReqMismatch: '{ id: number; raw: unknown }',
      C_ReqAgree: '{ id: string; raw: unknown }',
      C_Gql: '{ id: string; raw: string }',
      C_Plain: '{ id: string }',
      C_Ctor: '{ make: new () => object; raw: unknown }',
    });
    for (const alias of ['P_AnyMismatch', 'P_AnyAgree']) {
      setFindings(producer, alias, [{ path: 'raw', kind: 'any', reason: 'unresolved_import' }]);
    }
    setFindings(producer, 'P_Budget', [
      { path: 'deep', kind: 'budget_exhausted', reason: 'declared' },
    ]);
    const result = await runCheck({
      stubs: [
        { service_name: 'om-producer', stub_dir: producer.stub_dir },
        { service_name: 'om-consumer', stub_dir: consumer.stub_dir },
      ],
      pairs: [
        pair('agree', 'P_Agree', 'C_Agree'),
        pair('unknown-mismatch', 'P_UnknownMismatch', 'C_UnknownMismatch'),
        pair('unknown-agree', 'P_UnknownAgree', 'C_UnknownAgree'),
        pair('any-mismatch', 'P_AnyMismatch', 'C_AnyMismatch'),
        pair('any-agree', 'P_AnyAgree', 'C_AnyAgree'),
        pair('nested-mismatch', 'P_NestedMismatch', 'C_NestedMismatch'),
        pair('nested-agree', 'P_NestedAgree', 'C_NestedAgree'),
        pair('root-record', 'P_RootRecord', 'C_RootRecord'),
        pair('root-list', 'P_RootList', 'C_RootList'),
        pair('budget', 'P_Budget', 'C_Budget'),
        pair('request-mismatch', 'P_ReqExpects', 'C_ReqMismatch', request),
        pair('request-agree', 'P_ReqExpects2', 'C_ReqAgree', request),
        pair('graphql', 'P_Gql', 'C_Gql', { protocol: 'graphql' }),
        pair('plain', 'P_Plain', 'C_Plain'),
        pair('ctor', 'P_Ctor', 'C_Ctor'),
      ],
    });
    assert.strictEqual(result.success, true, JSON.stringify(result.errors));
    verdicts = new Map(result.verdicts.map((v) => [v.pair_key, v]));
  });

  after(() => {
    fs.rmSync(root, { recursive: true, force: true });
  });

  it('each fixture carries the capture finding it is about', () => {
    assert.deepStrictEqual(findingsOf(consumer, 'C_Agree'), [
      { path: 'meta', kind: 'unknown', reason: 'declared' },
    ]);
    assert.deepStrictEqual(findingsOf(producer, 'P_UnknownMismatch'), [
      { path: 'raw', kind: 'unknown', reason: 'declared' },
    ]);
    assert.deepStrictEqual(findingsOf(producer, 'P_UnknownAgree'), [
      { path: 'raw', kind: 'unknown', reason: 'declared' },
    ]);
    assert.deepStrictEqual(findingsOf(producer, 'P_NestedMismatch').map((f) => f.path), [
      'items<0>.ev',
    ]);
    assert.deepStrictEqual(findingsOf(producer, 'P_RootRecord').map((f) => f.path), ['[index]']);
    assert.deepStrictEqual(findingsOf(producer, 'P_RootList').map((f) => f.path), ['<0>']);
    assert.deepStrictEqual(findingsOf(consumer, 'C_ReqMismatch').map((f) => f.path), ['raw']);
    assert.deepStrictEqual(findingsOf(producer, 'P_Gql').map((f) => f.path), ['raw']);
    assert.deepStrictEqual(findingsOf(producer, 'P_Plain'), []);
    assert.deepStrictEqual(findingsOf(consumer, 'C_Plain'), []);
  });

  it('an agreement on the rest reads what the pre-gate wrote (consumer member open)', () => {
    assert.deepStrictEqual(
      withoutId(verdicts.get('agree')!),
      preGated('agree', 'consumer', 'unknown', 'meta')
    );
  });

  it('a mismatch on the rest is incompatible, not a fact, and names only the fields that differ', () => {
    const v = verdicts.get('unknown-mismatch')!;
    assert.strictEqual(v.bucket, 'incompatible', JSON.stringify(v));
    assert.strictEqual(v.resolved, false);
    assert.strictEqual(v.unresolved_side, 'producer');
    assert.match(v.unresolved_reason ?? '', /'raw'/);
    assert.match(v.diagnostic ?? '', /'id'/);
    assert.doesNotMatch(v.diagnostic ?? '', /raw/, v.diagnostic);
    assert.match(v.diagnostic ?? '', /P_UnknownMismatch/, v.diagnostic);
    assert.doesNotMatch(v.diagnostic ?? '', /OpenRoot|OpenMember|'Sent'/, v.diagnostic);
  });

  it('the sent side\'s open member alone files nothing: the pair reads what the pre-gate wrote', () => {
    assert.deepStrictEqual(
      withoutId(verdicts.get('unknown-agree')!),
      preGated('unknown-agree', 'producer', 'unknown', 'raw')
    );
  });

  it('an `any` member from an unresolved import: a mismatch files, an agreement keeps its gate', () => {
    const v = verdicts.get('any-mismatch')!;
    assert.strictEqual(v.bucket, 'incompatible', JSON.stringify(v));
    assert.strictEqual(v.resolved, false);
    assert.match(v.diagnostic ?? '', /'id'/);
    assert.deepStrictEqual(
      withoutId(verdicts.get('any-agree')!),
      preGated('any-agree', 'producer', 'any', 'raw')
    );
  });

  it('an open member inside a list element is opened where it sits', () => {
    const v = verdicts.get('nested-mismatch')!;
    assert.strictEqual(v.bucket, 'incompatible', JSON.stringify(v));
    assert.match(v.diagnostic ?? '', /'n'/, v.diagnostic);
    assert.deepStrictEqual(
      withoutId(verdicts.get('nested-agree')!),
      preGated('nested-agree', 'producer', 'unknown', 'items<0>.ev')
    );
  });

  it('a top type at no named member, or an exhausted budget, stays pre-gated in today\'s words', () => {
    assert.deepStrictEqual(
      withoutId(verdicts.get('root-record')!),
      preGated('root-record', 'producer', 'unknown', '[index]')
    );
    assert.deepStrictEqual(
      withoutId(verdicts.get('root-list')!),
      preGated('root-list', 'producer', 'unknown', '<0>')
    );
    assert.deepStrictEqual(
      withoutId(verdicts.get('budget')!),
      preGated('budget', 'producer', 'budget_exhausted', 'deep')
    );
  });

  it('a request body: the consumer sends, and its open member is the one opened', () => {
    const v = verdicts.get('request-mismatch')!;
    assert.strictEqual(v.bucket, 'incompatible', JSON.stringify(v));
    assert.strictEqual(v.unresolved_side, 'consumer');
    assert.doesNotMatch(v.diagnostic ?? '', /raw/, v.diagnostic);
    assert.deepStrictEqual(
      withoutId(verdicts.get('request-agree')!),
      preGated('request-agree', 'consumer', 'unknown', 'raw')
    );
  });

  it('a mismatch the opening alone makes files nothing: the declared types must disagree too', () => {
    assert.deepStrictEqual(
      withoutId(verdicts.get('ctor')!),
      preGated('ctor', 'producer', 'unknown', 'raw')
    );
  });

  it('a GraphQL pair stays pre-gated', () => {
    assert.deepStrictEqual(
      withoutId(verdicts.get('graphql')!),
      preGated('graphql', 'producer', 'unknown', 'raw')
    );
  });

  it('a pair with no finding is judged as before', () => {
    const v = verdicts.get('plain')!;
    assert.strictEqual(v.bucket, 'incompatible', JSON.stringify(v));
    assert.strictEqual(v.resolved, true);
    const spec = pair('plain', 'P_Plain', 'C_Plain');
    const plan = buildProbe(spec, (s) => `@carrick/${s}`);
    assert.strictEqual(plan.openAssignmentLine, undefined);
    assert.doesNotMatch(plan.source, /Open/);
  });
});

describe('a pair compared on the rest keeps its capture verdict when the check cannot run (carrick#1398)', () => {
  it('reads what the pre-gate wrote when isolation is unavailable', async () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-1398-early-'));
    try {
      const producer = captureLiterals(root, 'om-producer', { P: '{ id: number; raw: unknown }' });
      const consumer = captureLiterals(root, 'om-consumer', { C: '{ id: string; raw: string }' });
      const result = await runCheck({
        stubs: [
          { service_name: 'om-producer', stub_dir: producer.stub_dir },
          { service_name: 'om-consumer', stub_dir: consumer.stub_dir },
        ],
        pairs: [
          {
            pair_key: 'held',
            protocol: 'http',
            type_kind: 'response',
            producer: { service_name: 'om-producer', alias: 'P' },
            consumer: { service_name: 'om-consumer', alias: 'C' },
          },
        ],
        pnpmPath: path.join(root, 'no-pnpm-here'),
      });
      assert.strictEqual(result.isolation, 'unavailable');
      assert.deepStrictEqual(
        result.verdicts.map(withoutId),
        [preGated('held', 'producer', 'unknown', 'raw')]
      );
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  });
});

describe('the pre-gate\'s member rule matches the manifest state\'s (carrick#1398)', () => {
  it('follows the shared case table', () => {
    const table = JSON.parse(
      fs.readFileSync(
        new URL('../../test/fixtures/path-names-a-member.json', import.meta.url),
        'utf8'
      )
    ) as { cases: Array<[string, boolean]> };
    assert.ok(table.cases.length > 0);
    for (const [p, expected] of table.cases) {
      assert.strictEqual(pathNamesAMember(p), expected, `path ${JSON.stringify(p)}`);
    }
  });
});
