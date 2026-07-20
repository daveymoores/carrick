/**
 * v2 check — GraphQL resolver-return envelope unwrap (end-to-end, real
 * vendored pnpm + tsc).
 *
 * A GraphQL producer's captured type is the resolver's RETURN type with
 * transport layers already peeled (`Promise<ApiResponse<Order>>` ->
 * `{ data: Order; errors: string[] }`), while the consumer's expectation is
 * the SDL field payload its selection set reads (`OrderView`). Comparing the
 * envelope raw was the corpus-1 `graphql|query|order` false-incompatible
 * ("missing the following properties from type 'OrderView': id, total"); the
 * probe now unwraps an unambiguous single-payload envelope for graphql pairs
 * (v1 ts_check `unwrapGraphqlPayload` parity, ported type-level).
 *
 * Pins the fix AND its fail-closed edges: a real field mismatch under the
 * same envelope stays incompatible, a bare payload with an optional-vs-
 * required widening (the corpus-1 subscription shape) stays incompatible
 * with its field-level diagnostic, an ambiguous envelope is never unwrapped,
 * and the unwrap is graphql-scoped (the same envelope under http keeps its
 * raw comparison).
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { runCheck } from '../src/capture/index.js';
import type { CheckPairSpec, CheckStubInput, CheckVerdict } from '../src/capture/api.js';

let root: string;
let stubs: CheckStubInput[];

function writeStub(dir: string, serviceName: string, surface: string): void {
  const stubDir = path.join(dir, serviceName);
  fs.mkdirSync(path.join(stubDir, 'types'), { recursive: true });
  fs.writeFileSync(
    path.join(stubDir, 'package.json'),
    JSON.stringify(
      {
        name: `@carrick/${serviceName}`,
        version: '0.0.0-carrick',
        private: true,
        types: './types/surface.d.ts',
      },
      null,
      2
    ) + '\n'
  );
  fs.writeFileSync(path.join(stubDir, 'types', 'surface.d.ts'), surface);
}

function gql(pair_key: string, producerAlias: string, consumerAlias: string): CheckPairSpec {
  return {
    pair_key,
    protocol: 'graphql',
    type_kind: 'response',
    producer: { service_name: 'gateway', alias: producerAlias },
    consumer: { service_name: 'web', alias: consumerAlias },
  };
}

const PAIRS: CheckPairSpec[] = [
  // The corpus-1 `graphql|query|order` shape: envelope producer, subset consumer.
  gql('gql-envelope-subset', 'Env_Subset_Sent', 'Env_Subset_Exp'),
  // Inverse: a REAL field-type mismatch under the same envelope.
  gql('gql-envelope-mismatch', 'Env_Mismatch_Sent', 'Env_Mismatch_Exp'),
  // Bare payload, consumer selects a subset: plain assignability, no unwrap needed.
  gql('gql-bare-subset', 'Bare_Subset_Sent', 'Bare_Subset_Exp'),
  // The corpus-1 `graphql|subscription|orderUpdated` shape: bare payload with
  // an object-typed prop AND a union-of-objects prop; optional `note` vs
  // required. The unwrap must NOT misfire onto the single object prop.
  gql('gql-bare-widening', 'Bare_Widening_Sent', 'Bare_Widening_Exp'),
  // Two payload-shaped properties: ambiguous envelope, never unwrapped.
  gql('gql-envelope-ambiguous', 'Env_Ambiguous_Sent', 'Env_Ambiguous_Exp'),
  // Protocol scope: the same envelope shape under http keeps the raw compare.
  {
    pair_key: 'http-envelope-scoped',
    protocol: 'http',
    type_kind: 'response',
    producer: { service_name: 'gateway', alias: 'Env_Http_Sent' },
    consumer: { service_name: 'web', alias: 'Env_Http_Exp' },
  },
];

function byKey(verdicts: CheckVerdict[]): Map<string, CheckVerdict> {
  return new Map(verdicts.map((v) => [v.pair_key, v]));
}

describe('check_v2 graphql envelope unwrap (real pnpm + tsc)', () => {
  let verdicts: Map<string, CheckVerdict>;
  let rerun: CheckVerdict[];
  let first: CheckVerdict[];

  before(async () => {
    root = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-check-v2-gql-'));
    writeStub(
      root,
      'gateway',
      [
        // Corpus-1 resolveOrder: Promise-unwrapped ApiResponse<Order> envelope.
        'export type Env_Subset_Sent = { data: { id: string; total: { amountCents: number; currency: string }; status: { kind: "placed"; placedAt: string } | { kind: "refunded"; refundedAt: string; reason?: string }; note?: string }; errors: string[] };',
        // Same envelope, but total.amountCents is a string: a real wire break.
        'export type Env_Mismatch_Sent = { data: { id: string; total: { amountCents: string; currency: string }; note?: string }; errors: string[] };',
        'export type Bare_Subset_Sent = { id: string; total: { amountCents: number; currency: string }; note?: string };',
        // Corpus-1 Order: object prop (total) + union-of-objects prop (status).
        'export type Bare_Widening_Sent = { id: string; total: { amountCents: number; currency: string }; status: { kind: "placed"; placedAt: string } | { kind: "refunded"; refundedAt: string }; note?: string };',
        'export type Env_Ambiguous_Sent = { data: { id: string }; meta: { traceId: string }; errors: string[] };',
        'export type Env_Http_Sent = { data: { id: string }; errors: string[] };',
      ].join('\n') + '\n'
    );
    writeStub(
      root,
      'web',
      [
        'export type Env_Subset_Exp = { id: string; total: { amountCents: number; currency: string }; note?: string };',
        'export type Env_Mismatch_Exp = { id: string; total: { amountCents: number; currency: string }; note?: string };',
        'export type Bare_Subset_Exp = { id: string };',
        'export type Bare_Widening_Exp = { id: string; total: { amountCents: number; currency: string }; note: string };',
        'export type Env_Ambiguous_Exp = { id: string };',
        'export type Env_Http_Exp = { id: string };',
      ].join('\n') + '\n'
    );
    stubs = [
      { service_name: 'gateway', stub_dir: path.join(root, 'gateway') },
      { service_name: 'web', stub_dir: path.join(root, 'web') },
    ];
    const result = await runCheck({ stubs, pairs: PAIRS });
    assert.strictEqual(result.success, true, JSON.stringify(result.errors));
    assert.strictEqual(result.install_ok, true);
    first = result.verdicts;
    verdicts = byKey(result.verdicts);
    const second = await runCheck({ stubs, pairs: PAIRS });
    assert.strictEqual(second.success, true, JSON.stringify(second.errors));
    rerun = second.verdicts;
  });

  after(() => {
    fs.rmSync(root, { recursive: true, force: true });
  });

  it('envelope producer vs subset consumer -> compatible (the corpus-1 query fix)', () => {
    const v = verdicts.get('gql-envelope-subset')!;
    assert.strictEqual(v.bucket, 'compatible');
    assert.strictEqual(v.diagnostic, undefined);
  });

  it('real field mismatch under the same envelope -> incompatible, payload-level diagnostic', () => {
    const v = verdicts.get('gql-envelope-mismatch')!;
    assert.strictEqual(v.bucket, 'incompatible');
    assert.ok(v.diagnostic!.includes('amountCents'), v.diagnostic);
  });

  it('bare payload subset selection stays compatible (no unwrap needed)', () => {
    assert.strictEqual(verdicts.get('gql-bare-subset')!.bucket, 'compatible');
  });

  it('optional-vs-required widening on a bare payload stays incompatible with its field diagnostic (no misfire onto the single object prop)', () => {
    const v = verdicts.get('gql-bare-widening')!;
    assert.strictEqual(v.bucket, 'incompatible');
    // The union-of-objects sibling (status) blocks single-payload selection, so
    // the diagnostic elaborates the real `note` widening, not a bogus unwrap.
    assert.ok(v.diagnostic!.includes("'note'"), v.diagnostic);
  });

  it('ambiguous envelope (two payload-shaped props) is never unwrapped -> incompatible', () => {
    assert.strictEqual(verdicts.get('gql-envelope-ambiguous')!.bucket, 'incompatible');
  });

  it('the unwrap is graphql-scoped: the same envelope under http stays incompatible', () => {
    assert.strictEqual(verdicts.get('http-envelope-scoped')!.bucket, 'incompatible');
  });

  it('verdicts are byte-stable across two independent runs', () => {
    assert.strictEqual(JSON.stringify(first), JSON.stringify(rerun));
  });
});
