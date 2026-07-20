/**
 * Probe generation for the v2 check phase ("tsc as the judge").
 *
 * One probe file per matched pair. The probe is a value-level assignment in
 * the data-flow direction, guarded by six top/bottom-type gates on BOTH sides
 * (IsAny/IsUnknown/IsNever). Value-level (not `[X] extends [Y]`) because the
 * conditional-type relation diverges around `any`, and because the compiler's
 * elaborated assignment error is the user-facing mismatch report.
 *
 * Seam: node builtins + `typescript` + this bundle only. No imports needed here.
 */

import type {
  CheckPairEndpoint,
  CheckPairSpec,
  ProbeProtocol,
  ProbeTypeKind,
} from './api.js';

/** Which pair endpoint supplies the `sent` value / the `expected` binding. */
export type Side = 'producer' | 'consumer';

export interface Direction {
  sent: Side;
  expected: Side;
}

/**
 * The direction table, keyed on (protocol, type_kind) — one place. Structurally
 * fixes the confirmed HTTP request-body inversion (data flows consumer ->
 * producer for request bodies, so the check is consumer <= producer).
 *
 *  | protocol       | type_kind | sent     | expected |
 *  | http, graphql  | response  | producer | consumer |
 *  | http           | request   | consumer | producer |
 *  | socket, pubsub | both      | consumer | producer |
 */
export function directionFor(
  protocol: ProbeProtocol,
  typeKind: ProbeTypeKind
): Direction {
  if (protocol === 'socket' || protocol === 'pubsub') {
    return { sent: 'consumer', expected: 'producer' };
  }
  if (protocol === 'http' && typeKind === 'request') {
    return { sent: 'consumer', expected: 'producer' };
  }
  // http/graphql response, and any other (http, graphql) shape.
  return { sent: 'producer', expected: 'consumer' };
}

/** Deterministic FNV-1a (32-bit) over the pair's stable key. Never a path. */
export function fnv1a(input: string): string {
  let hash = 0x811c9dc5;
  for (let i = 0; i < input.length; i++) {
    hash ^= input.charCodeAt(i);
    // hash *= 16777619, kept in 32-bit unsigned range.
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(16).padStart(8, '0');
}

/** The pair ID is derived from the pair's semantic identity, not the workspace
 * path, so it is byte-stable across runs. */
export function pairId(spec: CheckPairSpec): string {
  const key = [
    spec.protocol,
    spec.type_kind,
    spec.producer.service_name,
    spec.producer.alias,
    spec.consumer.service_name,
    spec.consumer.alias,
    spec.pair_key,
  ].join('|');
  return fnv1a(key);
}

/** Names each gate line so a TS2344 can be attributed to a specific side+kind. */
export type GateName =
  | 'sent:any'
  | 'sent:unknown'
  | 'sent:never'
  | 'expected:any'
  | 'expected:unknown'
  | 'expected:never';

export interface ProbePlan {
  pairId: string;
  spec: CheckPairSpec;
  /** Probe file basename, e.g. pair_1a2b3c4d.ts */
  fileName: string;
  /** Which endpoint is sent vs expected (resolved via the direction table). */
  direction: Direction;
  sentEndpoint: CheckPairEndpoint;
  expectedEndpoint: CheckPairEndpoint;
  /** Generated probe source. */
  source: string;
  /** Lines the surface imports sit on (1-based). Errors here => unverifiable. */
  importLines: number[];
  /** 1-based line -> gate name. TS2344 here => baked-any / unverifiable. */
  gateLines: Map<number, GateName>;
  /** 1-based line of the value-level assignment. Errors here => incompatible. */
  assignmentLine: number;
}

/**
 * Build one probe, recording the exact line of every gate and the assignment so
 * the classifier never depends on hard-coded offsets. `packageOf` maps a
 * service name to its workspace package specifier (e.g. `@carrick/orders`).
 */
export function buildProbe(
  spec: CheckPairSpec,
  packageOf: (serviceName: string) => string
): ProbePlan {
  const id = pairId(spec);
  const direction = directionFor(spec.protocol, spec.type_kind);
  const sentEndpoint = direction.sent === 'producer' ? spec.producer : spec.consumer;
  const expectedEndpoint =
    direction.expected === 'producer' ? spec.producer : spec.consumer;

  const sentPkg = packageOf(sentEndpoint.service_name);
  const expectedPkg = packageOf(expectedEndpoint.service_name);

  const lines: string[] = [];
  const importLines: number[] = [];
  const gateLines = new Map<number, GateName>();

  const push = (text: string): number => {
    lines.push(text);
    return lines.length; // 1-based line number just written
  };

  importLines.push(
    push(`import type { ${sentEndpoint.alias} as Sent } from '${sentPkg}';`)
  );
  importLines.push(
    push(`import type { ${expectedEndpoint.alias} as Expected } from '${expectedPkg}';`)
  );
  push(`type IsAny<T> = 0 extends 1 & T ? true : false;`);
  push(`type IsUnknown<T> = unknown extends T ? (0 extends 1 & T ? false : true) : false;`);
  push(`type IsNever<T> = [T] extends [never] ? true : false;`);
  push(`type Not<T extends boolean> = T extends true ? false : true;`);
  push(`type Assert<T extends true> = T;`);
  gateLines.set(push(`type _G_sent_any = Assert<Not<IsAny<Sent>>>;`), 'sent:any');
  gateLines.set(push(`type _G_sent_unknown = Assert<Not<IsUnknown<Sent>>>;`), 'sent:unknown');
  gateLines.set(push(`type _G_sent_never = Assert<Not<IsNever<Sent>>>;`), 'sent:never');
  gateLines.set(push(`type _G_expected_any = Assert<Not<IsAny<Expected>>>;`), 'expected:any');
  gateLines.set(
    push(`type _G_expected_unknown = Assert<Not<IsUnknown<Expected>>>;`),
    'expected:unknown'
  );
  gateLines.set(push(`type _G_expected_never = Assert<Not<IsNever<Expected>>>;`), 'expected:never');
  push(`declare const sent: Sent;`);
  const assignmentLine = push(`const expected: Expected = sent;`);

  return {
    pairId: id,
    spec,
    fileName: `pair_${id}.ts`,
    direction,
    sentEndpoint,
    expectedEndpoint,
    source: lines.join('\n') + '\n',
    importLines,
    gateLines,
    assignmentLine,
  };
}
