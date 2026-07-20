/**
 * WP6: capture-fidelity corpus regression net.
 *
 * Runs the deterministic, LLM-free capture-fidelity harness over the fixed
 * corpus (fidelity-corpus.ts) and pins its surface-fidelity metric against a
 * checked-in baseline. This is the diff target future WP2/WP3 changes are read
 * against: a shift in any tier / self-check / anchor-origin count, or a
 * per-alias tier change, fails here with the computed metric printed and a
 * regen instruction.
 *
 * Deliberately NOT the corpus live eval: capture/check is deterministic given
 * anchors, so this whole suite runs locally at zero paid GCP/LLM spend. The
 * paid xrepo eval stays the separate regression net.
 *
 * To intentionally move the baseline after a capture-core change:
 *   UPDATE_FIDELITY_BASELINE=1 npm test
 * which rewrites test/fidelity-baseline.json from the current capture output.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import { SidecarClient } from './helpers.js';
import { runCaptureFidelity, serializeBaseline, type FidelityRun } from './fidelity-harness.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
// dist/test -> src/sidecar/test: the baseline is checked in beside the sources,
// read with fs at runtime (never `import`ed — resolveJsonModule would inline a
// compile-time copy and defeat the diff-against-checked-in-file purpose).
const BASELINE_PATH = path.join(__dirname, '..', '..', 'test', 'fidelity-baseline.json');
const UPDATE = process.env.UPDATE_FIDELITY_BASELINE === '1';

describe('capture fidelity corpus (WP6)', () => {
  const client = new SidecarClient();
  // Nullable: if before() throws before this is assigned, after() must not
  // rmSync an undefined path and mask the original failure.
  let outDir: string | undefined;
  let run: FidelityRun;
  let computed: string;

  before(async () => {
    outDir = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-fidelity-'));
    await client.start();
    run = await runCaptureFidelity(client, outDir);
    computed = serializeBaseline(run.baseline);
  });

  after(async () => {
    await client.stop();
    if (outDir) fs.rmSync(outDir, { recursive: true, force: true });
  });

  it('matches the checked-in fidelity baseline', () => {
    if (UPDATE) {
      fs.writeFileSync(BASELINE_PATH, computed);
      return;
    }
    assert.ok(
      fs.existsSync(BASELINE_PATH),
      `baseline missing at ${BASELINE_PATH}; regenerate with UPDATE_FIDELITY_BASELINE=1`,
    );
    const expected = fs.readFileSync(BASELINE_PATH, 'utf8');
    assert.strictEqual(
      computed,
      expected,
      'capture fidelity drifted from the checked-in baseline.\n' +
        'If this change to the capture core is intended, regenerate with:\n' +
        '  UPDATE_FIDELITY_BASELINE=1 npm test\n' +
        '--- computed ---\n' +
        computed,
    );
  });

  it('is byte-stable across two runs', async () => {
    assert.ok(outDir, 'outDir must be set by before()');
    const second = await runCaptureFidelity(client, outDir);
    assert.strictEqual(serializeBaseline(second.baseline), computed);
  });

  it('the corpus exercises every fidelity tier, outcome, and anchor origin', () => {
    // A degenerate corpus (e.g. all-emitted) would make the baseline blind to
    // regressions in the harder tiers; assert the aggregate spans the matrix.
    const agg = run.baseline.aggregate;
    for (const tier of ['emitted', 'node_builder', 'structural_fallback'] as const) {
      assert.ok(agg.by_serialization[tier] > 0, `no aliases at tier ${tier}`);
    }
    for (const outcome of ['ok', 'allowlisted_external', 'decayed_internal'] as const) {
      assert.ok(agg.by_self_check[outcome] > 0, `no aliases with self-check ${outcome}`);
    }
    for (const origin of ['llm-symbol', 'deterministic-infer', 'anchor-backfill'] as const) {
      assert.ok(agg.by_anchor_origin[origin] > 0, `no aliases from origin ${origin}`);
    }
  });

  it('aggregate is the exact sum of per-fixture counts', () => {
    const { fixtures, aggregate } = run.baseline;
    const sum = (pick: (f: (typeof fixtures)[number]) => number) =>
      fixtures.reduce((n, f) => n + pick(f), 0);
    assert.strictEqual(aggregate.total_aliases, sum((f) => f.total_aliases));
    assert.strictEqual(
      aggregate.by_serialization.emitted,
      sum((f) => f.by_serialization.emitted),
    );
    assert.strictEqual(
      aggregate.by_anchor_origin['llm-symbol'],
      sum((f) => f.by_anchor_origin['llm-symbol']),
    );
  });

  it('demotion reasons carry a diagnosable category (not baselined)', () => {
    // Free-text reasons are excluded from the byte-stable baseline; their
    // category is still pinned here so a silent reason regression is caught.
    const bare = run.raw.get('capture-v2-bare')!;
    const byAlias = new Map(bare.aliases.map((a) => [a.alias, a]));
    assert.match(byAlias.get('Endpoint_overloaded_Handler')!.capture_failure_reason ?? '', /overload/);
    assert.match(byAlias.get('Endpoint_generic_Handler')!.capture_failure_reason ?? '', /generic/);
  });
});
