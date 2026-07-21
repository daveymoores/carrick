/**
 * carrick#371 mirror-drift guard.
 *
 * The machinery-indicator set is intentionally DUPLICATED across the capture
 * seam: `type-inferrer.ts` (ts-morph, the v1 abstain path) and
 * `capture/machinery.ts` (raw `ts`, the capture demote path) each carry their
 * own copy, because the seam forbids sharing a module across it — a capture/
 * file may import only node builtins + `typescript` + its own bundle, and the
 * rest of the sidecar may reach the bundle only via `api.js`/`index.js`
 * (enforced by capture-v2-seam.test.ts). De-duplicating into a shared module
 * would break that boundary, so the two copies are kept in lockstep by THIS
 * test instead: if they drift, one detection path silently stops abstaining and
 * the carrick#371 false verdict can reappear on whichever path lost a member.
 */

import { describe, it } from 'node:test';
import * as assert from 'node:assert';
import { MACHINERY_MEMBER_INDICATORS as INFERRER_SET } from '../src/type-inferrer.js';
import { MACHINERY_MEMBER_INDICATORS as CAPTURE_SET } from '../src/capture/machinery.js';

describe('carrick#371 machinery-indicator mirror stays in lockstep', () => {
  it('the two duplicated indicator sets are byte-for-byte equal', () => {
    const inferrer = [...INFERRER_SET].sort();
    const capture = [...CAPTURE_SET].sort();
    assert.deepStrictEqual(
      capture,
      inferrer,
      'type-inferrer.ts and capture/machinery.ts MACHINERY_MEMBER_INDICATORS ' +
        'have drifted; update both copies together (see the doc comments)'
    );
  });

  it('the set is non-empty (a truncated copy must not read as "in sync")', () => {
    assert.ok(INFERRER_SET.size >= 3, 'indicator set unexpectedly small');
  });
});
