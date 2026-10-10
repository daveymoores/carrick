/**
 * v2 check core — abnormal tsc termination (adversarial-review finding 3).
 *
 * runCheck used to ignore the tsc exit code entirely: an OOM/SIGKILL
 * (code null), an internal crash, or a fileless config error (TS18003
 * prints WITHOUT a `(line,col)` prefix, so parseTscOutput yields zero
 * diagnostics) all fell through classifyPair's empty-diagnostics branch and
 * every pair read COMPATIBLE with success:true — the dangerous direction.
 *
 * These tests inject a stand-in tsc via CheckOptions.tscPath (the same
 * injection seam as pnpmPath) to pin:
 *  1. non-zero exit + zero parseable diagnostics -> every pair unverifiable,
 *     success:false, degraded services recorded;
 *  2. signal death (exit code null) -> same;
 *  3. non-zero exit WITH parseable probe diagnostics stays the legitimate
 *     incompatible path (tsc exits 1 on ordinary type errors — the guard
 *     must NOT be "fail on non-zero").
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { runCheck } from '../src/capture/index.js';
import { carriesHeapLimitLine, childNodeOptions } from '../src/capture/check.js';
import { buildProbe, decisiveAssignmentLine } from '../src/capture/check-probe.js';
import type { CheckPairSpec, CheckStubInput } from '../src/capture/api.js';

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

function fakeTsc(name: string, script: string): string {
  const file = path.join(root, name);
  fs.writeFileSync(file, `#!/bin/sh\n${script}\n`);
  fs.chmodSync(file, 0o755);
  return file;
}

const PAIRS: CheckPairSpec[] = [
  {
    pair_key: 'pair-a',
    protocol: 'http',
    type_kind: 'response',
    producer: { service_name: 'orders', alias: 'P_Res' },
    consumer: { service_name: 'web', alias: 'C_Res' },
  },
  {
    pair_key: 'pair-b',
    protocol: 'http',
    type_kind: 'request',
    producer: { service_name: 'orders', alias: 'P_Req' },
    consumer: { service_name: 'web', alias: 'C_Req' },
  },
];

describe('check_v2: abnormal tsc termination is never read as compatible', () => {
  before(() => {
    root = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-check-v2-tsc-'));
    writeStub(
      root,
      'orders',
      'export type P_Res = { a: string; };\nexport type P_Req = { a: string; };\n'
    );
    writeStub(
      root,
      'web',
      'export type C_Res = { a: string; };\nexport type C_Req = { a: string; };\n'
    );
    stubs = [
      { service_name: 'orders', stub_dir: path.join(root, 'orders') },
      { service_name: 'web', stub_dir: path.join(root, 'web') },
    ];
  });

  after(() => {
    fs.rmSync(root, { recursive: true, force: true });
  });

  it('crash exit with no parseable diagnostics -> all pairs unverifiable', async () => {
    const tscPath = fakeTsc(
      'tsc-crash',
      "echo 'error TS18003: No inputs were found in config file.' >&2; exit 1"
    );
    const result = await runCheck({ stubs, pairs: PAIRS, tscPath });
    assert.strictEqual(result.success, false);
    assert.strictEqual(result.install_ok, true);
    assert.strictEqual(result.verdicts.length, PAIRS.length);
    for (const v of result.verdicts) {
      assert.strictEqual(v.bucket, 'unverifiable', JSON.stringify(v));
      assert.strictEqual(v.gate, 'tsc:abnormal-termination');
      assert.match(v.diagnostic ?? '', /terminated abnormally/);
      // carrick#1833: the scanner reads each pair's own verdict on a failed
      // check, so the compiler's output rides the verdict, not only `errors`.
      assert.match(v.diagnostic ?? '', /exit code 1/);
      assert.match(v.diagnostic ?? '', /TS18003/);
      assert.strictEqual(v.unresolved_reason, v.diagnostic);
    }
    assert.strictEqual(result.degraded_services.length, 2);
    assert.match(result.errors.join('\n'), /exit code 1/);
    assert.match(result.errors.join('\n'), /TS18003/);
  });

  it('signal death (exit code null, e.g. OOM SIGKILL) -> all pairs unverifiable', async () => {
    const tscPath = fakeTsc('tsc-sigkill', 'kill -KILL $$');
    const result = await runCheck({ stubs, pairs: PAIRS, tscPath });
    assert.strictEqual(result.success, false);
    for (const v of result.verdicts) {
      assert.strictEqual(v.bucket, 'unverifiable', JSON.stringify(v));
      assert.strictEqual(v.gate, 'tsc:abnormal-termination');
      assert.match(v.diagnostic ?? '', /exit code null/);
    }
    assert.match(result.errors.join('\n'), /exit code null/);
  });

  it('a compiler that aborts on a full heap says so on the answer (carrick#2209)', async () => {
    const tscPath = fakeTsc(
      'tsc-heap',
      "echo 'FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory' >&2; kill -ABRT $$"
    );
    const result = await runCheck({ stubs, pairs: PAIRS, tscPath });
    assert.strictEqual(result.success, false);
    assert.deepStrictEqual(result.compiler_end, {
      exit_code: null,
      signal: 'SIGABRT',
      heap_limit: true,
    });
    for (const v of result.verdicts) {
      assert.strictEqual(v.bucket, 'unverifiable', JSON.stringify(v));
      assert.strictEqual(v.gate, 'tsc:abnormal-termination');
      assert.match(v.diagnostic ?? '', /exit code null/);
    }
  });

  it('a killed compiler is not a heap death, and a crash names its exit code', async () => {
    const killed = await runCheck({ stubs, pairs: PAIRS, tscPath: fakeTsc('tsc-kill', 'kill -KILL $$') });
    assert.deepStrictEqual(killed.compiler_end, {
      exit_code: null,
      signal: 'SIGKILL',
      heap_limit: false,
    });
    const crashed = await runCheck({
      stubs,
      pairs: PAIRS,
      tscPath: fakeTsc('tsc-crash-2', "echo 'error TS18003: No inputs.' >&2; exit 2"),
    });
    assert.deepStrictEqual(crashed.compiler_end, {
      exit_code: 2,
      signal: null,
      heap_limit: false,
    });
  });

  it('a compiler a signal ended after printing a diagnostic clears no pair (carrick#2209)', async () => {
    const plan = buildProbe(PAIRS[0], (s) => `@carrick/${s}`);
    const probeFile = `packages/carrick-probes/probes/${plan.fileName}`;
    const tscPath = fakeTsc(
      'tsc-partial-abort',
      `echo "${probeFile}(${decisiveAssignmentLine(plan)},7): error TS2322: Type 'A' is not assignable to type 'B'."; kill -ABRT $$`
    );
    const result = await runCheck({ stubs, pairs: PAIRS, tscPath });
    assert.strictEqual(result.success, false);
    assert.strictEqual(result.compiler_end?.signal, 'SIGABRT');
    for (const v of result.verdicts) {
      assert.strictEqual(v.bucket, 'unverifiable', JSON.stringify(v));
      assert.strictEqual(v.gate, 'tsc:abnormal-termination');
    }
  });

  it('the compiler runs under the heap cap the sidecar was given (carrick#2209)', () => {
    // The sidecar gets its cap as a node flag, which a child does not inherit;
    // the compiler must see it through NODE_OPTIONS. A fresh node with a cap
    // runs the check against a compiler that prints its NODE_OPTIONS.
    const tscPath = fakeTsc('tsc-env', 'echo "options: $NODE_OPTIONS" >&2; exit 1');
    const checkModule = pathToFileURL(
      path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'capture', 'index.js')
    ).href;
    const script =
      `const { runCheck } = await import(${JSON.stringify(checkModule)});` +
      `const r = await runCheck(${JSON.stringify({ stubs, pairs: PAIRS, tscPath })});` +
      'process.stdout.write(r.errors.join("\\n"));';
    const run = spawnSync(
      process.execPath,
      ['--max-old-space-size=1234', '--input-type=module', '-e', script],
      { encoding: 'utf8', env: { ...process.env, NODE_OPTIONS: '' } }
    );
    assert.strictEqual(run.status, 0, run.stderr);
    assert.match(run.stdout, /options: --max-old-space-size=1234/);
  });

  it('non-zero exit WITH parseable probe diagnostics stays the incompatible path', async () => {
    // Fake tsc emits a real assignment-class diagnostic for pair-a's probe
    // and exits 1 exactly like the real tsc does on ordinary type errors.
    // The abnormal-termination guard must not swallow it.
    const plan = buildProbe(PAIRS[0], (s) => `@carrick/${s}`);
    const probeFile = `packages/carrick-probes/probes/${plan.fileName}`;
    const tscPath = fakeTsc(
      'tsc-type-error',
      `echo "${probeFile}(${decisiveAssignmentLine(plan)},7): error TS2322: Type 'A' is not assignable to type 'B'."; exit 1`
    );
    const result = await runCheck({ stubs, pairs: PAIRS, tscPath });
    assert.strictEqual(result.success, true);
    const verdictA = result.verdicts.find((v) => v.pair_key === 'pair-a')!;
    assert.strictEqual(verdictA.bucket, 'incompatible', JSON.stringify(verdictA));
    const verdictB = result.verdicts.find((v) => v.pair_key === 'pair-b')!;
    assert.strictEqual(verdictB.bucket, 'compatible', JSON.stringify(verdictB));
  });
});

describe('check_v2: heap-death evidence and the compiler heap cap', () => {
  it('every heap-exhaustion line V8 prints is a heap death', () => {
    for (const line of [
      'FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory',
      'FATAL ERROR: Ineffective mark-compacts near heap limit Allocation failed - JavaScript heap out of memory',
      'FATAL ERROR: MarkCompactCollector: young object promotion failed Allocation failed - JavaScript heap out of memory',
      'FATAL ERROR: Scavenger: semi-space copy Allocation failed - JavaScript heap out of memory',
    ]) {
      assert.strictEqual(carriesHeapLimitLine(`<--- Last few GCs --->\n${line}\n 1: 0x1 node`), true, line);
    }
  });

  it('a hard size limit, or no line at all, is not', () => {
    for (const line of [
      'FATAL ERROR: invalid table size Allocation failed - JavaScript heap out of memory',
      'FATAL ERROR: invalid array length Allocation failed - JavaScript heap out of memory',
      'FATAL ERROR: Invalid array buffer length Allocation failed - JavaScript heap out of memory',
      'error TS18003: No inputs were found in config file.',
      '',
    ]) {
      assert.strictEqual(carriesHeapLimitLine(line), false, line);
    }
  });

  it("the child's NODE_OPTIONS carries the sidecar's own heap cap", () => {
    assert.strictEqual(childNodeOptions(['--max-old-space-size=8192'], undefined), '--max-old-space-size=8192');
    assert.strictEqual(
      childNodeOptions(['--max-old-space-size=8192'], '--enable-source-maps'),
      '--enable-source-maps --max-old-space-size=8192'
    );
    assert.strictEqual(childNodeOptions(['--max_old_space_size', '4096'], ''), '--max-old-space-size=4096');
    assert.strictEqual(childNodeOptions([], '--enable-source-maps'), '--enable-source-maps');
    assert.strictEqual(childNodeOptions(['--inspect'], undefined), undefined);
  });
});
