/**
 * Version 2 of the per-position rows (carrick#2430): `impl` rows name what
 * implements a call's bodyless declaration, and `jsx` rows record each JSX
 * element's tag.
 *
 * The fixture (`test/fixtures/position-rows-impl/`) is an interface `Port`
 * implemented by a class and by an object literal, an abstract class with two
 * subclasses, a function-typed constructor parameter, an interface with more
 * implementations than the cap, and two `.tsx` files with an imported
 * component and an intrinsic element. `v1-kinds.expected.jsonl` is what the
 * version-1 walk wrote for it: the rows of the version-1 kinds must not move.
 */
import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import * as zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { SidecarClient, asVersion1Rows } from './helpers.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const FIXTURE = path.join(__dirname, '..', '..', 'test', 'fixtures', 'position-rows-impl');

const FLAG_CHECKER_THREW = 4;
const FLAG_TOO_MANY = 16;

type Row = [string, number, number, string | null, string | null, number | null, string | null, string | null, number];

interface Walked {
  text: string;
  rows: Map<string, Row[]>;
  sources: Map<string, string>;
  counts: Record<string, number>;
}

/** Walk every source file of a copy of the fixture at `repo`, in path order. */
async function walk(repo: string, out: string): Promise<Walked> {
  const files = fs
    .readdirSync(path.join(repo, 'src'))
    .filter((file) => !file.endsWith('.d.ts'))
    .sort()
    .map((file) => path.join(repo, 'src', file));
  const client = new SidecarClient();
  await client.start();
  try {
    const ready = await client.send<{ status: string }>({
      action: 'init',
      request_id: 'init',
      repo_root: repo,
      tsconfig_path: 'tsconfig.json',
      scan_root: repo,
    });
    assert.strictEqual(ready.status, 'ready');
    const answer = await client.send<{ status: string; errors?: string[]; result?: Record<string, number> }>(
      { action: 'position_rows', request_id: 'rows', files, scan_root: repo, out_path: out, service: 'svc' },
      60_000
    );
    assert.strictEqual(answer.status, 'success', JSON.stringify(answer.errors));
    const text = zlib.gunzipSync(fs.readFileSync(out)).toString('utf8');
    const rows = new Map<string, Row[]>();
    const sources = new Map<string, string>();
    for (const line of text.trimEnd().split('\n').slice(1)) {
      const file = JSON.parse(line) as { f: string; r: Row[] };
      rows.set(file.f, file.r);
      sources.set(file.f, fs.readFileSync(path.join(repo, file.f), 'utf8'));
    }
    return { text, rows, sources, counts: answer.result ?? {} };
  } finally {
    await client.stop();
  }
}

describe('position_rows version 2: impl and jsx rows (carrick#2430)', () => {
  let root: string;
  let one: Walked;
  let two: Walked;

  before(async () => {
    root = fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-rows-impl-')));
    fs.mkdirSync(path.join(root, 'store'));
    const make = (name: string): Promise<Walked> => {
      const repo = path.join(root, 'trees', name, 'checkout');
      fs.cpSync(path.join(FIXTURE, 'repo'), repo, { recursive: true });
      return walk(repo, path.join(root, 'store', `${name}.jsonl.gz`));
    };
    one = await make('a');
    two = await make('b-longer-name');
  });

  after(() => {
    fs.rmSync(root, { recursive: true, force: true });
  });

  /** The rows of `kind` in `file` whose source text is exactly `text`. */
  const rowsAt = (file: string, kind: string, text: string): Row[] => {
    const source = one.sources.get(file) ?? '';
    return (one.rows.get(file) ?? []).filter((r) => r[0] === kind && source.slice(r[1], r[2]) === text);
  };

  /** `[symbol, decl_file, decl_line, decl_package, type_text, flags]` of each `impl` row at a span. */
  const implsAt = (file: string, text: string): unknown[][] =>
    rowsAt(file, 'impl', text).map((r) => [r[3], r[4], r[5], r[6], r[7], r[8]]);

  it('names the class and the object literal that implement an interface method, at the call and at the member', () => {
    const expected = [
      ['send', 'src/port.ts', 6, 'svc', null, 0],
      ['send', 'src/port.ts', 14, 'svc', null, 0],
    ];
    assert.deepStrictEqual(implsAt('src/main.ts', "port.send('created', { id: 1 })"), expected);
    assert.deepStrictEqual(implsAt('src/main.ts', 'port.send'), expected);
    assert.deepStrictEqual(implsAt('src/relay.ts', 'workerPort.send(name, body)'), expected);
  });

  it('names both subclasses of an abstract method', () => {
    assert.deepStrictEqual(implsAt('src/main.ts', "store.save('key', 'value')"), [
      ['save', 'src/store.ts', 8, 'svc', null, 0],
      ['save', 'src/store.ts', 14, 'svc', null, 0],
    ]);
  });

  it('writes one too_many row past the cap, with no list', () => {
    assert.deepStrictEqual(implsAt('src/main.ts', 'step.run()'), [[null, null, null, null, null, FLAG_TOO_MANY]]);
    assert.deepStrictEqual(implsAt('src/main.ts', 'step.run'), [[null, null, null, null, null, FLAG_TOO_MANY]]);
  });

  it('writes no impl row where the declaration has a body', () => {
    assert.deepStrictEqual(implsAt('src/main.ts', "relay.forward('moved', { id: 2 })"), []);
    assert.deepStrictEqual(implsAt('src/port.ts', "fetch(`/events/${name}`, { method: 'POST', body: JSON.stringify(body) })"), []);
  });

  it('writes what go-to-implementation answers for a function-typed constructor parameter: nothing', () => {
    // The language service names no implementation of a parameter; the
    // argument passed to the constructor is a value flow, not an implementation.
    assert.deepStrictEqual(implsAt('src/relay.ts', 'this.deliver(name, body)'), []);
  });

  it('never names a declaration as its own implementation', () => {
    // Go-to-implementation on an overloaded default-library method answers
    // with its own overload signatures.
    assert.deepStrictEqual(implsAt('src/port.ts', 'JSON.stringify(body)'), []);
  });

  it('never marks an impl search that answered as a checker throw', () => {
    for (const rows of one.rows.values()) {
      for (const r of rows) assert.ok(r[0] !== 'impl' || (r[8] & FLAG_CHECKER_THREW) === 0, JSON.stringify(r));
    }
  });

  it('writes a jsx row for each element: the imported component and the intrinsic', () => {
    const tag = (file: string, text: string): unknown[][] =>
      rowsAt(file, 'jsx', text).map((r) => [r[3], r[4], r[5], r[6]]);
    assert.deepStrictEqual(tag('src/view.tsx', 'Badge'), [['Badge', 'src/badge.tsx', 1, 'svc']]);
    assert.deepStrictEqual(tag('src/view.tsx', 'section'), [['section', 'src/jsx.d.ts', 6, 'svc']]);
    assert.deepStrictEqual(tag('src/badge.tsx', 'section'), [['section', 'src/jsx.d.ts', 6, 'svc']]);
    // An intrinsic reads as the attributes its tag is given, never the error type.
    assert.deepStrictEqual(
      rowsAt('src/view.tsx', 'jsx', 'section').map((r) => [r[7], r[8]]),
      [['{ title?: string | undefined; }', 0]]
    );
    const all = [...one.rows.values()].flat().filter((r) => r[0] === 'jsx');
    assert.strictEqual(all.length, 3, 'one jsx row per opening or self-closing element');
  });

  it('counts the impl and jsx rows it wrote', () => {
    const all = [...one.rows.values()].flat();
    assert.strictEqual(one.counts.impl, all.filter((r) => r[0] === 'impl').length);
    assert.strictEqual(one.counts.jsx, all.filter((r) => r[0] === 'jsx').length);
    assert.strictEqual(one.counts.rows, all.length);
  });

  it('leaves every row of a version-1 kind byte-identical', () => {
    const expected = fs.readFileSync(path.join(FIXTURE, 'v1-kinds.expected.jsonl'), 'utf8');
    assert.strictEqual(asVersion1Rows(one.text), expected);
  });

  it('writes byte-identical rows from two checkout paths', () => {
    assert.strictEqual(one.text, two.text);
  });
});
