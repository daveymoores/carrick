/**
 * carrick#2230 part 2: under CARRICK_UNTRUSTED_TREE the compiler reads no file
 * whose real path is outside the checkout.
 *
 * A scan on a machine that scans repositories it does not own reads only the
 * repository it was given. The fixture is generated here: a directory beside
 * the checkout holds a file carrying a sentinel, and the checkout reaches it
 * three ways. A tracked symbolic link, a relative import that climbs out, and
 * a tsconfig `include` that names the outside directory by absolute path. It
 * also holds an installed package whose link resolves INSIDE the checkout,
 * which must keep reading.
 *
 * The known positive comes first: with the switch unset the sentinel is read
 * (the capture publishes it, the loader's project holds it). Without that, a
 * refusal and a collapse look alike.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { captureStub } from '../src/capture/index.js';
import { ProjectLoader } from '../src/project-loader.js';
import { UNTRUSTED_TREE_ENV } from '../src/capture/checkout.js';

const SENTINEL = 'SENTINEL_9f3c_outside';
const PKG_MARK = 'PKG_MARK_inside_link';

function walkText(dir: string): string {
  let out = '';
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const abs = path.join(dir, entry.name);
    out += entry.isDirectory() ? walkText(abs) : fs.readFileSync(abs, 'utf8') + '\n';
  }
  return out;
}

describe('carrick#2230: an untrusted scan reads inside the checkout only', () => {
  let base: string;
  let checkout: string;
  let outside: string;
  const saved = process.env[UNTRUSTED_TREE_ENV];

  const write = (rel: string, text: string): void => {
    fs.mkdirSync(path.dirname(path.join(checkout, rel)), { recursive: true });
    fs.writeFileSync(path.join(checkout, rel), text);
  };

  before(() => {
    base = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-2230b-'));
    outside = path.join(base, 'outside');
    checkout = path.join(base, 'checkout');
    fs.mkdirSync(outside, { recursive: true });
    fs.writeFileSync(
      path.join(outside, 'secret.ts'),
      `export interface Secret { ${SENTINEL}: string }\n`
    );
    fs.mkdirSync(path.join(checkout, '.git'), { recursive: true });
    write('package.json', JSON.stringify({ name: 'svc', private: true }));
    write(
      'tsconfig.json',
      JSON.stringify({
        compilerOptions: { target: 'ES2022', module: 'ESNext', moduleResolution: 'Bundler', strict: true, skipLibCheck: true },
        include: ['src', outside],
      })
    );
    fs.mkdirSync(path.join(checkout, 'src'), { recursive: true });
    fs.symlinkSync(path.join(outside, 'secret.ts'), path.join(checkout, 'src/link.ts'));
    write(
      'src/uses.ts',
      [
        "import type { Secret } from '../../outside/secret';",
        "import type { Secret as Linked } from './link';",
        "import type { Pkg } from 'pkg';",
        'export interface Wrapped { own: string; s: Secret; l: Linked; p: Pkg }',
        '',
      ].join('\n')
    );
    // An installed package: node_modules/pkg is a link that resolves inside the checkout.
    write('vendor/pkg/package.json', JSON.stringify({ name: 'pkg', version: '1.0.0', types: 'index.d.ts' }));
    write('vendor/pkg/index.d.ts', `export interface Pkg { ${PKG_MARK}: number }\n`);
    fs.mkdirSync(path.join(checkout, 'node_modules'), { recursive: true });
    fs.symlinkSync(path.join(checkout, 'vendor/pkg'), path.join(checkout, 'node_modules/pkg'));
  });

  after(() => {
    if (saved === undefined) delete process.env[UNTRUSTED_TREE_ENV];
    else process.env[UNTRUSTED_TREE_ENV] = saved;
    fs.rmSync(base, { recursive: true, force: true });
  });

  const capture = (name: string): { text: string; anyPaths: string[] } => {
    const outDir = path.join(base, `stub-${name}`);
    const result = captureStub({
      repoRoot: checkout,
      serviceName: name,
      outDir,
      anchors: [
        {
          kind: 'symbol',
          alias: 'Endpoint_svc_Response',
          symbol_name: 'Wrapped',
          source_file: 'src/uses.ts',
          anchor_origin: 'llm-symbol',
        },
      ],
    });
    const anyPaths = (result.aliases?.[0]?.any_provenance ?? []).map((p) => p.path).sort();
    return { text: JSON.stringify(result) + '\n' + walkText(outDir), anyPaths };
  };

  const wrappedMembers = (): Record<string, string> => {
    const loader = new ProjectLoader({ repoRoot: checkout });
    assert.ok(loader.load().success);
    const iface = loader
      .getProject()
      .getSourceFileOrThrow(path.join(checkout, 'src/uses.ts'))
      .getInterfaceOrThrow('Wrapped');
    return Object.fromEntries(
      iface.getProperties().map((p) => [p.getName(), p.getType().isAny() ? 'any' : p.getType().getText()])
    );
  };

  it('1. switch unset: the outside file is read (the known positive)', () => {
    delete process.env[UNTRUSTED_TREE_ENV];
    const open = capture('open');
    assert.ok(open.text.includes(SENTINEL), 'capture publishes the outside type');
    assert.deepStrictEqual(open.anyPaths, []);
    const members = wrappedMembers();
    assert.notStrictEqual(members.s, 'any');
    assert.notStrictEqual(members.l, 'any');
  });

  it('2. switch set: the capture publishes nothing from outside and still reads the installed package', () => {
    process.env[UNTRUSTED_TREE_ENV] = '1';
    const { text, anyPaths } = capture('bound');
    assert.ok(!text.includes(SENTINEL), 'sentinel reached the capture');
    assert.ok(text.includes('own: string'), 'the checkout is read');
    // The outside members read as unresolved; the package behind a link that
    // resolves inside the checkout does not.
    assert.deepStrictEqual(anyPaths, ['l', 's']);
  });

  it('3. switch set: the loader reads outside paths as missing, so their types are any', () => {
    process.env[UNTRUSTED_TREE_ENV] = '1';
    const members = wrappedMembers();
    assert.strictEqual(members.s, 'any');
    assert.strictEqual(members.l, 'any');
    assert.strictEqual(members.own, 'string');
    assert.match(members.p, /Pkg/, 'the installed package keeps its type');
    const loader = new ProjectLoader({ repoRoot: checkout });
    loader.load();
    for (const file of loader.getProject().getSourceFiles()) {
      assert.ok(!file.getFullText().includes(SENTINEL), `${file.getFilePath()} holds the sentinel`);
    }
  });
});
