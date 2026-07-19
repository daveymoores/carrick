/**
 * SPIKE runner: v2 capture on the bare inventory-svc fixture, end to end.
 *
 *   cd src/sidecar && npm run build && node dist/src/spike/run-capture-spike.js
 *
 * Phase 1 (capture): run captureStub against
 * tests/fixtures/xrepo-corpus-3/inventory-svc (a bare checkout — the fixture
 * has no node_modules) and print the emitted stub, showing third-party
 * annotations preserved as import references.
 *
 * Phase 2 (check-side smoke, needs network for one `npm install` into the
 * stub): install the stub's pinned deps, generate v2-style probe files, run
 * `tsc --noEmit`, and classify diagnostics into
 * compatible / incompatible / unverifiable — demonstrating
 *   - a clean pair verdicts compatible,
 *   - a genuinely mismatched pair verdicts incompatible with the compiler's
 *     elaborated error as the report,
 *   - the alias that decayed to `any` on the bare checkout is caught by the
 *     probe gates (TS2344) and verdicts unverifiable, never compatible.
 */

import ts from 'typescript';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { execSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { captureStub } from '../capture-v2.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
// dist/src/spike -> repo root is five levels up
const REPO_ROOT = path.join(__dirname, '..', '..', '..', '..', '..');
const FIXTURE = path.join(REPO_ROOT, 'tests', 'fixtures', 'xrepo-corpus-3', 'inventory-svc');

const GATE_PRELUDE = `
type IsAny<T> = 0 extends 1 & T ? true : false;
type IsUnknown<T> = unknown extends T ? (0 extends 1 & T ? false : true) : false;
type IsNever<T> = [T] extends [never] ? true : false;
type Not<T extends boolean> = T extends true ? false : true;
type Assert<T extends true> = T;
`;

function probe(sentImport: string, expectedDecl: string): string {
  return `${sentImport}\n${expectedDecl}\n${GATE_PRELUDE}
type _G1 = Assert<Not<IsAny<Sent>>>;
type _G2 = Assert<Not<IsUnknown<Sent>>>;
type _G3 = Assert<Not<IsNever<Sent>>>;
type _G4 = Assert<Not<IsAny<Expected>>>;
type _G5 = Assert<Not<IsUnknown<Expected>>>;
type _G6 = Assert<Not<IsNever<Expected>>>;

declare const sent: Sent;
const expected: Expected = sent;
`;
}

function classify(fileName: string, diags: readonly ts.Diagnostic[]): string {
  const own = diags.filter((d) => d.file && path.basename(d.file.fileName) === fileName);
  if (own.some((d) => d.code === 2344)) return 'unverifiable (gate fired: a side is any/unknown/never)';
  if (own.some((d) => [2322, 2559, 2739, 2741].includes(d.code))) return 'incompatible';
  if (own.length > 0) return `unverifiable (unexpected: ${own.map((d) => d.code).join(',')})`;
  return 'compatible';
}

function main(): void {
  console.log('=== Phase 1: capture (bare checkout) ===');
  console.log(`fixture: ${FIXTURE}`);
  console.log(`fixture node_modules present: ${fs.existsSync(path.join(FIXTURE, 'node_modules'))}`);

  const workDir = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-v2-spike-'));
  const stubDir = path.join(workDir, 'stub');
  const result = captureStub({
    repoRoot: FIXTURE,
    serviceName: 'inventory-svc',
    outDir: stubDir,
    anchors: [
      { alias: 'Endpoint_stock_Response', symbol_name: 'StockLevel', source_file: 'src/types/stock.ts', anchor_origin: 'llm-symbol' },
      { alias: 'Sub_stockadjust_Payload', symbol_name: 'StockAdjustCommand', source_file: 'src/types/stock.ts', anchor_origin: 'llm-symbol' },
      { alias: 'Sub_priceupdated_Payload', symbol_name: 'PriceUpdatedEvent', source_file: 'src/types/stock.ts', anchor_origin: 'deterministic-infer' },
    ],
  });

  console.log(JSON.stringify({ ...result, stub_dir: '<tmp>/stub' }, null, 2));
  for (const rel of result.emitted_files) {
    console.log(`\n--- ${rel} ---`);
    console.log(fs.readFileSync(path.join(stubDir, rel), 'utf8').trimEnd());
  }
  if (!result.success) process.exit(1);

  console.log('\n=== Phase 2: check-side smoke (probes against the stub) ===');
  console.log(`npm install into stub (pinned: ${JSON.stringify(result.pinned_dependencies)}) ...`);
  execSync('npm install --no-audit --no-fund --loglevel=error', { cwd: stubDir, stdio: 'inherit' });

  const probesDir = path.join(workDir, 'probes');
  fs.mkdirSync(probesDir);
  const surface = path
    .relative(probesDir, path.join(stubDir, 'types', 'surface'))
    .split(path.sep)
    .join('/');

  const probes: Record<string, string> = {
    // Compatible: consumer expects exactly what the producer sends.
    'pair_ok.ts': probe(
      `import type { Endpoint_stock_Response as Sent } from '${surface}';`,
      `type Expected = { sku: string; warehouseId: string; onHand: number; reserved: number };`
    ),
    // Incompatible: consumer requires a field the producer does not send.
    'pair_mismatch.ts': probe(
      `import type { Endpoint_stock_Response as Sent } from '${surface}';`,
      `type Expected = { sku: string; warehouseId: string; onHand: number; reserved: number; restockAt: string };`
    ),
    // Gate: the zod-inferred alias baked to any on the bare checkout —
    // must classify unverifiable, never compatible.
    'pair_gate.ts': probe(
      `import type { Sub_stockadjust_Payload as Sent } from '${surface}';`,
      `type Expected = { sku: string; delta: number; reason: string; orderId: string };`
    ),
  };
  for (const [name, text] of Object.entries(probes)) {
    fs.writeFileSync(path.join(probesDir, name), text);
  }

  const program = ts.createProgram(
    Object.keys(probes).map((f) => path.join(probesDir, f)),
    {
      noEmit: true,
      strict: true,
      skipLibCheck: true,
      noUnusedLocals: false,
      module: ts.ModuleKind.ESNext,
      moduleResolution: ts.ModuleResolutionKind.Bundler,
      types: [],
    }
  );
  const diags = ts.getPreEmitDiagnostics(program);

  for (const name of Object.keys(probes)) {
    console.log(`\n${name}: ${classify(name, diags)}`);
    for (const d of diags.filter((d) => d.file && path.basename(d.file.fileName) === name)) {
      const { line } = ts.getLineAndCharacterOfPosition(d.file!, d.start!);
      console.log(
        `  TS${d.code} @ line ${line + 1}: ` +
          ts.flattenDiagnosticMessageText(d.messageText, '\n    ').slice(0, 500)
      );
    }
  }

  console.log(`\n(work dir kept for inspection: ${workDir})`);
}

main();
