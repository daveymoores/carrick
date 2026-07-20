/**
 * Diagnostic parsing + four-bucket classification for the v2 check phase.
 *
 * The judge is the vendored `tsc` CLI with `--pretty false`, run from the
 * workspace root so file locations print workspace-relative (no temp path in
 * the location prefix). This module turns that text into per-pair verdicts,
 * classifying by diagnostic code + file + line (never by line position alone):
 *
 *   poison (stub-file diagnostic)         -> unverifiable   [highest precedence]
 *   surface import error (probe lines 1-2)-> unverifiable
 *   IsAny gate fired (TS2344)             -> gate_caught_baked_any
 *   IsUnknown/IsNever gate fired (TS2344) -> unverifiable
 *   assignment-class error                -> incompatible
 *   no diagnostics                        -> compatible     [lowest precedence]
 *
 * Gate precedence over the assignment line is load-bearing: an `unknown` side
 * produces BOTH a gate TS2344 and an assignment TS2322, and reading the latter
 * would mislabel an unverifiable pair as incompatible.
 *
 * Seam: node builtins + this bundle only.
 */

import type { CheckVerdict } from './api.js';
import type { GateName, ProbePlan, Side } from './check-probe.js';
import { scrubDiagnostic, type ScrubContext } from './check-scrub.js';

export interface RawDiagnostic {
  /** Workspace-relative, forward-slash file path (empty for global errors). */
  file: string;
  line: number;
  col: number;
  code: number;
  /** Primary text plus any indented elaboration lines, joined with '\n'. */
  message: string;
}

const PRIMARY_RE =
  /^(?<file>(?:[a-zA-Z]:)?[^(]*?)\((?<line>\d+),(?<col>\d+)\): error TS(?<code>\d+): (?<msg>.*)$/;

/** Parse `tsc --pretty false` output into structured diagnostics. */
export function parseTscOutput(stdout: string): RawDiagnostic[] {
  const diags: RawDiagnostic[] = [];
  let current: RawDiagnostic | null = null;
  for (const rawLine of stdout.split('\n')) {
    const line = rawLine.replace(/\r$/, '');
    const m = line.match(PRIMARY_RE);
    if (m && m.groups) {
      current = {
        file: m.groups.file.split('\\').join('/'),
        line: Number(m.groups.line),
        col: Number(m.groups.col),
        code: Number(m.groups.code),
        message: m.groups.msg,
      };
      diags.push(current);
      continue;
    }
    // Indented continuation lines belong to the preceding primary diagnostic.
    if (current && /^\s+\S/.test(line)) {
      current.message += '\n' + line;
      continue;
    }
    // Blank line or summary ("Found N errors.") ends the current run.
    current = null;
  }
  return diags;
}

/** Assignment-class codes: a real structural mismatch on the value assignment. */
const ASSIGNMENT_CODES = new Set([2322, 2559, 2739, 2740, 2741, 2769, 2345]);

function sideForGate(name: GateName, plan: ProbePlan): { side: Side; kind: string } {
  const [which, kind] = name.split(':');
  const side = which === 'sent' ? plan.direction.sent : plan.direction.expected;
  return { side, kind };
}

function endpointAliasFor(side: Side, plan: ProbePlan): string {
  return side === plan.direction.sent
    ? plan.sentEndpoint.alias
    : plan.expectedEndpoint.alias;
}

export interface ClassifyInput {
  plan: ProbePlan;
  /** Diagnostics attributed to this pair's probe file. */
  probeDiags: RawDiagnostic[];
  /** Returns a reason string if the service's stub tree is poisoned. */
  poisonReason: (serviceName: string) => string | undefined;
  scrubCtx: ScrubContext;
}

/** Classify one pair into exactly one bucket, honouring the precedence order. */
export function classifyPair(input: ClassifyInput): CheckVerdict {
  const { plan, probeDiags, poisonReason, scrubCtx } = input;
  const codes = [...new Set(probeDiags.map((d) => d.code))].sort((a, b) => a - b);
  const base = { pair_id: plan.pairId, pair_key: plan.spec.pair_key, codes };

  // 1. Poison: any diagnostic in either side's stub tree makes the pair
  //    unverifiable, never "no probe error -> compatible".
  for (const side of ['producer', 'consumer'] as const) {
    const service = plan.spec[side].service_name;
    const reason = poisonReason(service);
    if (reason) {
      return {
        ...base,
        bucket: 'unverifiable',
        gate: `poison:${side}`,
        diagnostic: `types for service '${service}' contain declaration conflicts; compatibility cannot be verified.`,
      };
    }
  }

  // 2. Surface import error (missing/renamed export): probe import lines.
  const importDiag = probeDiags.find((d) => plan.importLines.includes(d.line));
  if (importDiag) {
    const side: Side = plan.importLines[0] === importDiag.line ? plan.direction.sent : plan.direction.expected;
    const alias = endpointAliasFor(side, plan);
    return {
      ...base,
      bucket: 'unverifiable',
      gate: `import:${side}`,
      diagnostic: `surface export '${alias}' for the ${side} is missing or renamed; compatibility cannot be verified.`,
    };
  }

  // 3/4. Probe gates (TS2344). IsAny outranks IsUnknown/IsNever.
  const gateDiags = probeDiags.filter(
    (d) => d.code === 2344 && plan.gateLines.has(d.line)
  );
  const anyGate = gateDiags
    .map((d) => plan.gateLines.get(d.line)!)
    .find((name) => name.endsWith(':any'));
  if (anyGate) {
    const { side } = sideForGate(anyGate, plan);
    return {
      ...base,
      bucket: 'gate_caught_baked_any',
      gate: `${side}:any`,
      diagnostic: `the ${side} type resolved to 'any' at check time; compatibility cannot be verified (a type inferred through a missing library bakes to any).`,
    };
  }
  const decayGate = gateDiags
    .map((d) => plan.gateLines.get(d.line)!)
    .find((name) => name.endsWith(':unknown') || name.endsWith(':never'));
  if (decayGate) {
    const { side, kind } = sideForGate(decayGate, plan);
    return {
      ...base,
      bucket: 'unverifiable',
      gate: `${side}:${kind}`,
      diagnostic: `the ${side} type resolved to '${kind}' at check time; compatibility cannot be verified.`,
    };
  }

  // 5. Assignment-class error on the value assignment line -> incompatible.
  const assignDiag = probeDiags.find(
    (d) => d.line === plan.assignmentLine && ASSIGNMENT_CODES.has(d.code)
  );
  if (assignDiag) {
    return {
      ...base,
      bucket: 'incompatible',
      diagnostic: scrubDiagnostic(
        assignDiag.message,
        scrubCtx,
        plan.sentEndpoint.alias,
        plan.expectedEndpoint.alias
      ),
    };
  }

  // Any other diagnostic on the assignment line that is not a known assignment
  // code still means the pair could not be cleanly verified.
  const otherAssign = probeDiags.find((d) => d.line === plan.assignmentLine);
  if (otherAssign) {
    return {
      ...base,
      bucket: 'unverifiable',
      gate: 'assignment:other',
      diagnostic: scrubDiagnostic(
        otherAssign.message,
        scrubCtx,
        plan.sentEndpoint.alias,
        plan.expectedEndpoint.alias
      ),
    };
  }

  // 6. No diagnostics -> compatible.
  return { ...base, bucket: 'compatible' };
}
