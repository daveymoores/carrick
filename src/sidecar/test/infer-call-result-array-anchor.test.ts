/**
 * #336: a `call_result` inference that resolves to an array type must anchor
 * on the ELEMENT symbol and report the peeled depth, exactly as the producer
 * paths do since #306. The live repro is `axios.get<Order[]>(...)`: the LLM
 * anchor is the bare element (`Order`), so the explicit bundle pre-claims the
 * consumer alias, and without a depth reported here `apply_inferred_array_depth`
 * (Rust) has nothing to copy onto the `SymbolRequest` and the bundler renders
 * `export interface <alias> {...}` with the `[]` gone. The producer keeps
 * `Order[]`, so every scan reports a false array-vs-scalar contract risk.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as path from 'node:path';
import * as fs from 'node:fs';
import { SidecarClient, FIXTURES_PATH } from './helpers.js';

const FIXTURE = path.join(FIXTURES_PATH, 'src/wrapper-usage.ts');
const FIXTURE_SOURCE = fs.readFileSync(FIXTURE, 'utf-8');

/** Byte span of `text` in the fixture (ASCII-only file, so byte == char offsets). */
function spanOf(text: string): { start: number; end: number; line: number } {
  const start = FIXTURE_SOURCE.indexOf(text);
  assert.ok(start >= 0, `fixture must contain: ${text}`);
  assert.strictEqual(
    FIXTURE_SOURCE.indexOf(text, start + 1),
    -1,
    `fixture must contain exactly one occurrence of: ${text}`
  );
  const line = FIXTURE_SOURCE.slice(0, start).split('\n').length;
  return { start, end: start + text.length, line };
}

interface InferResponseShape {
  request_id: string;
  status: string;
  inferred_types?: Array<{
    alias: string;
    type_string: string;
    infer_kind: string;
    is_explicit: boolean;
    primary_type_symbol?: string;
    array_depth?: number;
  }>;
  errors?: string[];
}

describe('call_result array payloads anchor on the element symbol with array_depth (#336)', () => {
  let client: SidecarClient;

  before(async () => {
    client = new SidecarClient();
    await client.start();
    await client.send({
      action: 'init',
      request_id: 'call-arr-init',
      repo_root: FIXTURES_PATH,
    });
  });

  after(async () => {
    await client.stop();
  });

  it('explicit generic `apiGet<UserData[]>` unwrapped by a rule → anchor UserData, depth 1', async () => {
    const call = spanOf("apiGet<UserData[]>('/api/users')");
    const response = await client.send<InferResponseShape>({
      action: 'infer',
      request_id: 'call-arr-wrapped',
      extraction_config: {
        rules: [
          {
            wrapperSymbols: ['ApiResponse'],
            originModuleGlobs: ['wrapper-lib', 'wrapper-lib/*'],
            payloadGenericIndex: 0,
          },
        ],
      },
      requests: [
        {
          file_path: FIXTURE,
          line_number: call.line,
          span_start: call.start,
          span_end: call.end,
          infer_kind: 'call_result',
          alias: 'UserArrayConsumer',
        },
      ],
    });

    const inferred = response.inferred_types?.find(
      (t) => t.alias === 'UserArrayConsumer'
    );
    assert.ok(
      inferred,
      `expected an inferred type, got errors: ${JSON.stringify(response.errors)}`
    );
    assert.strictEqual(
      inferred.primary_type_symbol,
      'UserData',
      `anchor must be the element symbol, got ${JSON.stringify(inferred.primary_type_symbol)}`
    );
    assert.strictEqual(
      inferred.array_depth,
      1,
      'without the depth the explicit bundle for the same alias erases the []'
    );
  });

  it('unwrapped `Promise<UserData[]>` call → anchor UserData, depth 1', async () => {
    const call = spanOf('await fetchUsersDirect()');
    const callStart = call.start + 'await '.length;
    const response = await client.send<InferResponseShape>({
      action: 'infer',
      request_id: 'call-arr-direct',
      requests: [
        {
          file_path: FIXTURE,
          line_number: call.line,
          span_start: callStart,
          span_end: call.end,
          infer_kind: 'call_result',
          alias: 'UserArrayDirectConsumer',
        },
      ],
    });

    const inferred = response.inferred_types?.find(
      (t) => t.alias === 'UserArrayDirectConsumer'
    );
    assert.ok(
      inferred,
      `expected an inferred type, got errors: ${JSON.stringify(response.errors)}`
    );
    assert.strictEqual(inferred.primary_type_symbol, 'UserData');
    assert.strictEqual(inferred.array_depth, 1);
  });

  it('scalar wrapper payload keeps its anchor and reports NO array_depth', async () => {
    const call = spanOf('client.fetchUser();\n  return resp.data');
    const response = await client.send<InferResponseShape>({
      action: 'infer',
      request_id: 'call-arr-scalar',
      extraction_config: {
        rules: [
          {
            wrapperSymbols: ['ApiResponse'],
            originModuleGlobs: ['wrapper-lib', 'wrapper-lib/*'],
            payloadGenericIndex: 0,
          },
        ],
      },
      requests: [
        {
          file_path: FIXTURE,
          line_number: call.line,
          span_start: call.start,
          span_end: call.start + 'client.fetchUser()'.length,
          infer_kind: 'call_result',
          alias: 'UserScalarConsumer',
        },
      ],
    });

    const inferred = response.inferred_types?.find(
      (t) => t.alias === 'UserScalarConsumer'
    );
    assert.ok(
      inferred,
      `expected an inferred type, got errors: ${JSON.stringify(response.errors)}`
    );
    assert.strictEqual(inferred.primary_type_symbol, 'UserData');
    assert.strictEqual(
      inferred.array_depth,
      undefined,
      `a scalar must not report an array_depth, got ${inferred.array_depth}`
    );
  });
});
