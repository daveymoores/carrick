/**
 * WP6 capture-fidelity harness.
 *
 * Drives the `capture_v2` stdio action over FIDELITY_CORPUS and rolls the
 * per-alias fidelity fields WP1 already emits (serialization tier, self-check
 * outcome, anchor origin) into a corpus-wide surface-fidelity metric, broken
 * out BY TIER and BY ANCHOR-ORIGIN, per fixture and in aggregate.
 *
 * Seam (pinned decision 11a): the harness consumes only the stdio-action
 * output and the public result JSON. It imports types from capture/api.js and
 * nothing from capture/ internals; it never calls captureStub directly.
 *
 * Determinism: fixtures are ordered by id, aliases by name, and every Record
 * is built with a fixed key order, so JSON.stringify is byte-stable run to run.
 * The baseline snapshot deliberately excludes anything path-, version-, or
 * toolchain-shaped (stub_dir, ts_version, pinned_dependencies, emitted_files,
 * and the free-text self_check_detail / capture_failure_reason). Those vary per
 * run or per TS bump; the harness returns the raw results alongside so tests
 * can still assert failure-reason categories without baselining them.
 */

import * as assert from 'node:assert';
import * as path from 'node:path';
import type { SidecarClient } from './helpers.js';
import { FIDELITY_CORPUS, type FidelityFixture } from './fidelity-corpus.js';
import type {
  AnchorOrigin,
  CaptureStubResult,
  SelfCheckOutcome,
  SerializationTier,
} from '../src/capture/api.js';

/**
 * Locale-independent code-unit comparator. `String.localeCompare` orders by the
 * runtime's default locale, which differs across machines (CI Linux vs macOS
 * ICU) and would make the baseline non-byte-stable; identifiers are ASCII so a
 * code-unit sort is total and portable.
 */
export function byCodeUnit(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

const TIERS: SerializationTier[] = ['emitted', 'node_builder', 'structural_fallback'];
const OUTCOMES: SelfCheckOutcome[] = ['ok', 'allowlisted_external', 'decayed_internal'];
const ORIGINS: AnchorOrigin[] = ['llm-symbol', 'deterministic-infer', 'anchor-backfill'];

/** Per-alias metric fields (structured only; no free-text, no paths). */
export interface AliasFidelity {
  alias: string;
  serialization: SerializationTier;
  self_check: SelfCheckOutcome;
  anchor_origin: AnchorOrigin;
  top_type_at_self_check: boolean;
}

export interface FidelityCounts {
  total_aliases: number;
  by_serialization: Record<SerializationTier, number>;
  by_self_check: Record<SelfCheckOutcome, number>;
  by_anchor_origin: Record<AnchorOrigin, number>;
  usable_rate: number;
}

export interface FixtureFidelity extends FidelityCounts {
  id: string;
  bare_checkout: boolean;
  aliases: AliasFidelity[];
}

export interface CorpusFidelity {
  fixtures: FixtureFidelity[];
  aggregate: FidelityCounts;
}

export interface FidelityRun {
  /** The byte-stable, baselined metric. */
  baseline: CorpusFidelity;
  /** Full public result per fixture id, for non-baselined category assertions. */
  raw: Map<string, CaptureStubResult>;
}

interface CaptureV2ResponseShape {
  request_id: string;
  status: string;
  result?: CaptureStubResult;
  errors?: string[];
}

function zeroed<T extends string>(keys: T[]): Record<T, number> {
  const out = {} as Record<T, number>;
  for (const k of keys) out[k] = 0;
  return out;
}

function usableRate(bySelfCheck: Record<SelfCheckOutcome, number>, total: number): number {
  if (total === 0) return 0;
  const usable = bySelfCheck.ok + bySelfCheck.allowlisted_external;
  return Math.round((usable / total) * 1000) / 1000;
}

function countsFromAliases(aliases: AliasFidelity[]): FidelityCounts {
  const bySer = zeroed(TIERS);
  const bySelf = zeroed(OUTCOMES);
  const byOrigin = zeroed(ORIGINS);
  for (const a of aliases) {
    bySer[a.serialization]++;
    bySelf[a.self_check]++;
    byOrigin[a.anchor_origin]++;
  }
  return {
    total_aliases: aliases.length,
    by_serialization: bySer,
    by_self_check: bySelf,
    by_anchor_origin: byOrigin,
    usable_rate: usableRate(bySelf, aliases.length),
  };
}

async function captureFixture(
  client: SidecarClient,
  fixture: FidelityFixture,
  outRoot: string,
): Promise<CaptureStubResult> {
  const response = (await client.send(
    {
      request_id: `fidelity-${fixture.id}`,
      action: 'capture_v2',
      repo_root: fixture.repoRoot,
      service_name: fixture.serviceName,
      // Stubs land in a caller-owned temp root, never inside the fixture tree.
      out_dir: path.join(outRoot, fixture.id),
      anchors: fixture.anchors,
    },
    // Compiler-heavy action: CI runners need far more than the 10s default.
    120000,
  )) as CaptureV2ResponseShape;

  assert.strictEqual(
    response.status,
    'success',
    `capture_v2 failed for ${fixture.id}: ${JSON.stringify(response.errors)}`,
  );
  assert.ok(response.result, `no result for ${fixture.id}`);
  return response.result;
}

function fixtureFidelity(fixture: FidelityFixture, result: CaptureStubResult): FixtureFidelity {
  const aliases: AliasFidelity[] = result.aliases
    .map((a) => ({
      alias: a.alias,
      serialization: a.serialization,
      self_check: a.self_check,
      anchor_origin: a.anchor_origin,
      top_type_at_self_check: a.top_type_at_self_check,
    }))
    .sort((a, b) => byCodeUnit(a.alias, b.alias));

  const counts = countsFromAliases(aliases);
  return {
    id: fixture.id,
    bare_checkout: result.bare_checkout,
    total_aliases: counts.total_aliases,
    by_serialization: counts.by_serialization,
    by_self_check: counts.by_self_check,
    by_anchor_origin: counts.by_anchor_origin,
    usable_rate: counts.usable_rate,
    aliases,
  };
}

function aggregate(fixtures: FixtureFidelity[]): FidelityCounts {
  const allAliases = fixtures.flatMap((f) => f.aliases);
  return countsFromAliases(allAliases);
}

/**
 * Run the whole fidelity corpus against an already-started client. The client
 * is warm-standby: one process serves all fixtures in order. `outRoot` is a
 * caller-owned (temp) directory the per-fixture stubs are written under.
 */
export async function runCaptureFidelity(
  client: SidecarClient,
  outRoot: string,
): Promise<FidelityRun> {
  const raw = new Map<string, CaptureStubResult>();
  const fixtures: FixtureFidelity[] = [];
  for (const fixture of FIDELITY_CORPUS) {
    const result = await captureFixture(client, fixture, outRoot);
    raw.set(fixture.id, result);
    fixtures.push(fixtureFidelity(fixture, result));
  }
  fixtures.sort((a, b) => byCodeUnit(a.id, b.id));
  return { baseline: { fixtures, aggregate: aggregate(fixtures) }, raw };
}

/**
 * Canonical serialization for the checked-in baseline. Two-space JSON with a
 * trailing newline; object key order is fixed by construction above.
 */
export function serializeBaseline(baseline: CorpusFidelity): string {
  return JSON.stringify(baseline, null, 2) + '\n';
}
