/**
 * v2 check core: "tsc as the judge"
 * (docs/reference/type-compat-synthetic-monorepo.md, Check phase).
 *
 * Consumes two or more capture stub packages, assembles a scratch synthetic
 * monorepo, installs pinned deps with the vendored pnpm (isolated linker),
 * generates one probe per matched pair, runs the vendored `tsc` CLI, and
 * classifies diagnostics into the four buckets. Deterministic given the stubs
 * and pairs: verdicts are byte-stable across runs (the scrub pass removes the
 * per-run temp path, and the pair IDs / ordering derive from the pair specs).
 *
 * Seam: this file imports only node builtins, `typescript` (for the version
 * string), and the rest of this bundle. The sidecar reaches it only through
 * ./index.js. The judge is the `tsc` CLI (no compiler API), which keeps the
 * check phase TS7-ready by construction.
 */

import { spawn } from 'node:child_process';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import type {
  CheckOptions,
  CheckProgressPhase,
  CheckResult,
  CheckVerdict,
  DegradedService,
} from './api.js';
import { buildProbe, type ProbePlan } from './check-probe.js';
import {
  classifyPair,
  parseTscOutput,
  type RawDiagnostic,
} from './check-classify.js';
import { scrubPaths, type ScrubContext } from './check-scrub.js';
import {
  assembleWorkspace,
  writeProbes,
  type AssembledWorkspace,
} from './check-workspace.js';

export type CheckProgress = (phase: CheckProgressPhase, message: string) => void;

interface SpawnResult {
  code: number | null;
  stdout: string;
  stderr: string;
}

function runProcess(
  command: string,
  args: string[],
  cwd: string
): Promise<SpawnResult> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d: Buffer) => (stdout += d.toString()));
    child.stderr.on('data', (d: Buffer) => (stderr += d.toString()));
    child.on('error', reject);
    child.on('close', (code) => resolve({ code, stdout, stderr }));
  });
}

/** Resolve the sidecar root (where the vendored pnpm/tsc live) from this bundle. */
function sidecarRoot(): string {
  // dist/src/capture/check.js -> up three -> sidecar root.
  const here = path.dirname(fileURLToPath(import.meta.url));
  return path.resolve(here, '..', '..', '..');
}

function binPath(name: string): string {
  return path.join(sidecarRoot(), 'node_modules', '.bin', name);
}

function unverifiableAll(
  plans: ProbePlan[],
  gate: string,
  diagnostic: string
): CheckVerdict[] {
  return plans.map((plan) => ({
    pair_id: plan.pairId,
    pair_key: plan.spec.pair_key,
    bucket: 'unverifiable' as const,
    gate,
    diagnostic,
    codes: [],
  }));
}

function sortVerdicts(verdicts: CheckVerdict[]): CheckVerdict[] {
  return [...verdicts].sort((a, b) =>
    a.pair_id < b.pair_id ? -1 : a.pair_id > b.pair_id ? 1 : 0
  );
}

/** Run the deterministic check phase. Async: install + tsc run off the event
 * loop so the sidecar stays responsive and can emit keepalive frames. */
export async function runCheck(
  opts: CheckOptions,
  onProgress?: CheckProgress
): Promise<CheckResult> {
  const progress: CheckProgress = onProgress ?? (() => {});
  const cleanup = opts.cleanup !== false;
  const pnpmPath = opts.pnpmPath ?? binPath('pnpm');
  const tscPath = binPath('tsc');
  const tsVersion = ts.version;

  // Isolation is non-negotiable: without the vendored pnpm we would fall back
  // to a flat npm install that manufactures nominal false-incompatibles. Fail
  // explicitly instead (design Check step 2).
  if (!fs.existsSync(pnpmPath)) {
    const planned = planPairs(opts, (s) => `@carrick/${s}`);
    return {
      success: false,
      workspace_dir: '',
      isolation: 'unavailable',
      install_ok: false,
      install_error: 'vendored pnpm not found; type isolation is unavailable',
      ts_version: tsVersion,
      verdicts: sortVerdicts(
        unverifiableAll(
          planned,
          'isolation:unavailable',
          'type isolation unavailable (pnpm missing); compatibility cannot be verified.'
        )
      ),
      degraded_services: opts.stubs.map((s) => ({
        service_name: s.service_name,
        reason: 'isolation unavailable',
      })),
      errors: ['vendored pnpm not found'],
    };
  }

  progress('assembling', 'assembling synthetic workspace');
  const ws = assembleWorkspace({
    stubs: opts.stubs,
    workspaceRoot: opts.workspaceRoot,
  });

  const scrubCtx: ScrubContext = {
    workspaceRoot: ws.workspaceDir,
    packageLabelOf: ws.packageLabelOf,
  };

  // Generate probes. A pair naming a service with no stub is unverifiable.
  const plans: ProbePlan[] = [];
  const unresolved: CheckVerdict[] = [];
  for (const spec of opts.pairs) {
    try {
      plans.push(buildProbe(spec, ws.packageOf));
    } catch {
      unresolved.push({
        pair_id: '',
        pair_key: spec.pair_key,
        bucket: 'unverifiable',
        gate: 'missing:stub',
        diagnostic: 'a stub for one side of this pair was not provided; compatibility cannot be verified.',
        codes: [],
      });
    }
  }
  // Backfill deterministic pair IDs for unresolved pairs from the spec.
  for (const v of unresolved) {
    const spec = opts.pairs.find((p) => p.pair_key === v.pair_key)!;
    v.pair_id = fnvOfSpec(spec);
  }
  writeProbes(ws, plans);

  const errors: string[] = [];
  const degraded: DegradedService[] = [];

  // ---- Install (async, off the event loop) --------------------------------
  progress('installing', 'installing pinned dependencies');
  const install = await runProcess(pnpmPath, ['install'], ws.workspaceDir);
  const installOk = install.code === 0;
  let installError: string | undefined;
  if (!installOk) {
    installError = scrubPaths(
      (install.stderr || install.stdout).trim().slice(0, 2000),
      scrubCtx
    );
    for (const s of opts.stubs) {
      degraded.push({ service_name: s.service_name, reason: 'workspace install failed' });
    }
    const verdicts = sortVerdicts([
      ...unverifiableAll(
        plans,
        'install:failed',
        'workspace dependency install failed; compatibility cannot be verified.'
      ),
      ...unresolved,
    ]);
    if (cleanup) safeRm(ws.workspaceDir);
    return {
      success: false,
      workspace_dir: cleanup ? '' : ws.workspaceDir,
      isolation: 'pnpm',
      install_ok: false,
      install_error: installError,
      ts_version: tsVersion,
      verdicts,
      degraded_services: degraded,
      errors,
    };
  }

  // ---- Judge (async, off the event loop) ----------------------------------
  progress('checking', 'type-checking probes');
  const tsc = await runProcess(
    tscPath,
    ['--noEmit', '--pretty', 'false', '-p', path.join(ws.probesRel, 'tsconfig.json')],
    ws.workspaceDir
  );
  const diagnostics = parseTscOutput(tsc.stdout + '\n' + tsc.stderr);

  const { probeDiagsByPair, poison, globalErrors } = attributeDiagnostics(
    diagnostics,
    ws,
    plans
  );
  for (const e of globalErrors) errors.push(scrubPaths(e, scrubCtx));
  for (const [service, reason] of poison) {
    degraded.push({ service_name: service, reason });
  }

  const verdicts = sortVerdicts([
    ...plans.map((plan) =>
      classifyPair({
        plan,
        probeDiags: probeDiagsByPair.get(plan.pairId) ?? [],
        poisonReason: (svc) => poison.get(svc),
        scrubCtx,
      })
    ),
    ...unresolved,
  ]);

  const workspaceDirOut = cleanup ? '' : ws.workspaceDir;
  if (cleanup) safeRm(ws.workspaceDir);

  return {
    success: true,
    workspace_dir: workspaceDirOut,
    isolation: 'pnpm',
    install_ok: true,
    ts_version: tsVersion,
    verdicts,
    degraded_services: dedupeDegraded(degraded),
    errors,
  };
}

/** Group diagnostics: per-probe (for classification), plus stub-tree poison. */
function attributeDiagnostics(
  diagnostics: RawDiagnostic[],
  ws: AssembledWorkspace,
  plans: ProbePlan[]
): {
  probeDiagsByPair: Map<string, RawDiagnostic[]>;
  poison: Map<string, string>;
  globalErrors: string[];
} {
  const probeFileToPair = new Map<string, string>();
  for (const plan of plans) {
    probeFileToPair.set(`${ws.probesRel}/probes/${plan.fileName}`, plan.pairId);
  }
  const stubDirs = new Set(ws.stubs.map((s) => s.packageDir));

  const probeDiagsByPair = new Map<string, RawDiagnostic[]>();
  const poison = new Map<string, string>();
  const globalErrors: string[] = [];

  for (const d of diagnostics) {
    if (!d.file) {
      globalErrors.push(`TS${d.code}: ${d.message}`);
      continue;
    }
    const pairId = probeFileToPair.get(d.file);
    if (pairId) {
      if (!probeDiagsByPair.has(pairId)) probeDiagsByPair.set(pairId, []);
      probeDiagsByPair.get(pairId)!.push(d);
      continue;
    }
    // Stub-tree diagnostics poison that stub (its OWN sources, not deps).
    const pkgMatch = d.file.match(/^packages\/([^/]+)\//);
    if (pkgMatch && !d.file.includes('/node_modules/') && stubDirs.has(pkgMatch[1])) {
      const service = ws.serviceOfPackageDir(pkgMatch[1]);
      if (service && !poison.has(service)) {
        poison.set(service, 'stub type tree carries its own diagnostics');
      }
    }
    // Anything else (node_modules, TS lib) is environmental: ignored for
    // verdicts so it cannot inject nondeterministic noise.
  }

  return { probeDiagsByPair, poison, globalErrors };
}

function dedupeDegraded(list: DegradedService[]): DegradedService[] {
  const seen = new Map<string, DegradedService>();
  for (const d of list) if (!seen.has(d.service_name)) seen.set(d.service_name, d);
  return [...seen.values()].sort((a, b) =>
    a.service_name < b.service_name ? -1 : a.service_name > b.service_name ? 1 : 0
  );
}

/** Plan pairs without a workspace (for the isolation-unavailable early exit). */
function planPairs(
  opts: CheckOptions,
  packageOf: (serviceName: string) => string
): ProbePlan[] {
  const plans: ProbePlan[] = [];
  for (const spec of opts.pairs) {
    try {
      plans.push(buildProbe(spec, packageOf));
    } catch {
      /* skipped in the early-exit path */
    }
  }
  return plans;
}

function fnvOfSpec(spec: CheckOptions['pairs'][number]): string {
  return buildProbe(spec, (s) => `@carrick/${s}`).pairId;
}

function safeRm(dir: string): void {
  try {
    fs.rmSync(dir, { recursive: true, force: true });
  } catch {
    /* best effort */
  }
}
