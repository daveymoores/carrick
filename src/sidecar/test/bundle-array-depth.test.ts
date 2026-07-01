/**
 * #248: a `SymbolRequest.array_depth > 0` wraps the bundled element type in that
 * many TS array levels. This is the GraphQL producer type-locate path: an SDL
 * field `orders: [Order!]!` backed by `interface Order` (with no resolver) is
 * bundled as `Order` with `array_depth: 1`, so the alias must resolve to the
 * element's fully-inlined body followed by `[]` — the shape from the element
 * type, the list depth from the SDL marker.
 *
 * Reuses UserProfile (test/fixtures/sample-repo/src/types.ts), the same nested
 * shape the explicit-symbol bundle test exercises, so we also confirm the wrap
 * does not break the structural inlining of nested named members.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as path from 'node:path';
import { SidecarClient, FIXTURES_PATH } from './helpers.js';

const TYPES_FIXTURE = path.join(FIXTURES_PATH, 'src/types.ts');
const noWs = (s: string) => s.replace(/\s/g, '');

interface BundleResponseShape {
  request_id: string;
  status: string;
  dts_content?: string;
  errors?: string[];
}

interface ResolveResponseShape {
  request_id: string;
  status: string;
  definitions?: Array<{ type_alias: string; definition: string; expanded: string }>;
  errors?: string[];
}

describe('array_depth wraps a bundled symbol in TS array levels (#248)', () => {
  let client: SidecarClient;

  before(async () => {
    client = new SidecarClient();
    await client.start();
    await client.send({ action: 'init', request_id: 'arr-init', repo_root: FIXTURES_PATH });
  });

  after(async () => {
    await client.stop();
  });

  it('emits `= {...}[];` (a type alias, not an interface) and keeps nested inlining', async () => {
    const ALIAS = 'Endpoint_arr1_Response';
    const bundle = await client.send<BundleResponseShape>({
      action: 'bundle',
      request_id: 'arr-bundle',
      symbols: [
        { symbol_name: 'UserProfile', source_file: TYPES_FIXTURE, alias: ALIAS, array_depth: 1 },
      ],
    });
    assert.strictEqual(bundle.status, 'success', `bundle failed: ${JSON.stringify(bundle.errors)}`);
    const dts = bundle.dts_content ?? '';

    // Arrays can't be interfaces: the alias must be a type alias, not `interface`.
    assert.ok(
      new RegExp(`type\\s+${ALIAS}\\s*=`).test(dts) && !dts.includes(`interface ${ALIAS}`),
      `array-wrapped alias must be a type alias, got:\n${dts}`,
    );
    // The RHS ends in `[]` and the element body is still structurally inlined.
    assert.ok(noWs(dts).includes('}[]'), `expected a trailing []-wrapped body, got:\n${dts}`);
    assert.ok(
      noWs(dts).includes('settings:{') && !/settings\s*:\s*UserSettings/.test(dts),
      `nested member must still be inlined under the wrap, got:\n${dts}`,
    );
  });

  it('resolves the wrapped alias to `{...}[]` end-to-end', async () => {
    const ALIAS = 'Endpoint_arr2_Response';
    const bundle = await client.send<BundleResponseShape>({
      action: 'bundle',
      request_id: 'arr-bundle-2',
      symbols: [
        { symbol_name: 'UserProfile', source_file: TYPES_FIXTURE, alias: ALIAS, array_depth: 1 },
      ],
    });
    assert.strictEqual(bundle.status, 'success');

    const resolved = await client.send<ResolveResponseShape>({
      action: 'resolve_definitions',
      request_id: 'arr-resolve',
      bundled_dts: bundle.dts_content ?? '',
      aliases: [ALIAS],
    });
    assert.strictEqual(resolved.status, 'success', `resolve failed: ${JSON.stringify(resolved.errors)}`);
    const def = resolved.definitions?.find((d) => d.type_alias === ALIAS);
    assert.ok(def, `expected a resolved definition for ${ALIAS}`);

    // The end-to-end expanded string (what the scorer compares) is the inlined
    // element body wrapped in a single array level.
    assert.ok(noWs(def.expanded).endsWith('[]'), `expanded must end in [], got: ${def.expanded}`);
    assert.ok(
      noWs(def.expanded).includes('settings:{') && !def.expanded.includes('UserSettings'),
      `expanded must inline nested members under the wrap, got: ${def.expanded}`,
    );
  });
});
