/**
 * SPIKE: v2 capture ("tsc as serializer") — bare-checkout stdio test.
 *
 * Drives the sidecar over the same JSON-over-stdio wire the Rust client uses
 * (the integration point for src/services/type_sidecar.rs), against the
 * xrepo-corpus-3 inventory-svc fixture, which has NO node_modules — the bare
 * checkout case from the v2 design study.
 *
 * What this pins:
 *  1. `tsc --noCheck --declaration --emitDeclarationOnly` capture succeeds on
 *     a bare checkout (exit clean, tree emitted).
 *  2. Third-party type ANNOTATIONS survive as syntactic import references
 *     (`import { z } from "zod"` + `z.infer<typeof StockAdjustSchema>`
 *     verbatim in the emitted tree) where expandTypeStructural bakes `any`.
 *  3. The amended self-check: the zod-dependent alias is ALLOWLISTED (kept at
 *     tier 'emitted', not decayed to Unknown) because its unresolved external
 *     specifier is pinned in the stub's dependencies.
 *  4. The honest limitation: types INFERRED through the missing library
 *     (`export declare const StockAdjustSchema: any`) bake to `any` on a bare
 *     checkout; the check-phase probe gates are the backstop for those.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { SidecarClient } from './helpers.js';
import type { CaptureStubResult } from '../src/capture-v2.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
// dist/test -> repo root is four levels up (dist/test -> dist -> sidecar -> src -> root)
const FIXTURE = path.join(
  __dirname, '..', '..', '..', '..',
  'tests', 'fixtures', 'xrepo-corpus-3', 'inventory-svc'
);

interface CaptureV2ResponseShape {
  request_id: string;
  status: string;
  result?: CaptureStubResult;
  errors?: string[];
}

describe('capture_v2 (spike): bare-checkout tsc-as-serializer', () => {
  const client = new SidecarClient();
  let outDir: string;
  let response: CaptureV2ResponseShape;

  before(async () => {
    assert.ok(fs.existsSync(FIXTURE), `fixture missing: ${FIXTURE}`);
    assert.ok(
      !fs.existsSync(path.join(FIXTURE, 'node_modules')),
      'precondition: fixture must be a bare checkout (no node_modules)'
    );
    outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-capture-v2-test-'));
    await client.start();
    response = (await client.send({
      request_id: 'capture-v2-spike-1',
      action: 'capture_v2',
      repo_root: FIXTURE,
      service_name: 'inventory-svc',
      out_dir: path.join(outDir, 'stub'),
      anchors: [
        {
          alias: 'Endpoint_stock_Response',
          symbol_name: 'StockLevel',
          source_file: 'src/types/stock.ts',
          anchor_origin: 'llm-symbol',
        },
        {
          alias: 'Sub_stockadjust_Payload',
          symbol_name: 'StockAdjustCommand',
          source_file: 'src/types/stock.ts',
          anchor_origin: 'llm-symbol',
        },
        {
          alias: 'Sub_priceupdated_Payload',
          symbol_name: 'PriceUpdatedEvent',
          source_file: 'src/types/stock.ts',
          anchor_origin: 'deterministic-infer',
        },
      ],
    })) as CaptureV2ResponseShape;
  });

  after(async () => {
    await client.stop();
    fs.rmSync(outDir, { recursive: true, force: true });
  });

  it('succeeds on a bare checkout with clean emit', () => {
    assert.strictEqual(response.status, 'success', JSON.stringify(response.errors));
    assert.ok(response.result);
    assert.strictEqual(response.result.bare_checkout, true);
    assert.deepStrictEqual(response.result.errors, []);
  });

  it('emits the v2 stub package shape', () => {
    const result = response.result!;
    assert.strictEqual(result.package_name, '@carrick/inventory-svc');
    assert.ok(result.emitted_files.includes('types/surface.d.ts'));
    assert.ok(result.emitted_files.includes('types/src/types/stock.d.ts'));
    assert.ok(fs.existsSync(path.join(result.stub_dir, 'package.json')));
    assert.ok(fs.existsSync(path.join(result.stub_dir, 'tsconfig.snapshot.json')));
    const pkg = JSON.parse(fs.readFileSync(path.join(result.stub_dir, 'package.json'), 'utf8'));
    assert.strictEqual(pkg.types, './types/surface.d.ts');
  });

  it('preserves third-party annotations as import references (not any)', () => {
    const result = response.result!;
    const stockDts = fs.readFileSync(
      path.join(result.stub_dir, 'types', 'src', 'types', 'stock.d.ts'),
      'utf8'
    );
    // The exact loss expandTypeStructural forces today: these survive verbatim.
    assert.ok(stockDts.includes('import { z } from "zod"'), stockDts);
    assert.ok(stockDts.includes('z.infer<typeof StockAdjustSchema>'), stockDts);
    // Honest limitation on bare checkouts: inference THROUGH the missing
    // library still bakes any (annotations survive; inferred consts do not).
    assert.ok(stockDts.includes('StockAdjustSchema: any'), stockDts);
  });

  it('pins referenced externals to exact lockfile versions', () => {
    const result = response.result!;
    assert.strictEqual(result.pinned_dependencies['zod'], '3.23.0');
    assert.deepStrictEqual(result.unpinned_externals, []);
    // Pruning: express/amqplib/nats are in the lockfile but not referenced
    // by the emitted tree, so they must not be pinned into the stub.
    assert.strictEqual(result.pinned_dependencies['express'], undefined);
  });

  it('self-check: plain internal types pass', () => {
    const byAlias = new Map(response.result!.aliases.map((a) => [a.alias, a]));
    for (const alias of ['Endpoint_stock_Response', 'Sub_priceupdated_Payload']) {
      const rec = byAlias.get(alias)!;
      assert.strictEqual(rec.self_check, 'ok', alias);
      assert.strictEqual(rec.top_type_at_self_check, false, alias);
      assert.strictEqual(rec.serialization, 'emitted');
    }
  });

  it('self-check: pinned-external failure is allowlisted, not decayed (amendment 1)', () => {
    const byAlias = new Map(response.result!.aliases.map((a) => [a.alias, a]));
    const rec = byAlias.get('Sub_stockadjust_Payload')!;
    assert.strictEqual(rec.self_check, 'allowlisted_external', rec.self_check_detail);
    assert.strictEqual(rec.top_type_at_self_check, true);
    // Still tier 'emitted': the alias is NOT downgraded to Unknown.
    assert.strictEqual(rec.serialization, 'emitted');
    assert.match(rec.self_check_detail ?? '', /zod@3\.23\.0/);
  });

  it('records anchor provenance separately from the serialization tier (amendment 2)', () => {
    const byAlias = new Map(response.result!.aliases.map((a) => [a.alias, a]));
    assert.strictEqual(byAlias.get('Endpoint_stock_Response')!.anchor_origin, 'llm-symbol');
    assert.strictEqual(
      byAlias.get('Sub_priceupdated_Payload')!.anchor_origin,
      'deterministic-infer'
    );
  });
});
