/**
 * A member whose symbol has no declaration gets its `impl` row from
 * go-to-implementation asked at the use site's name (carrick#2430, #2441).
 *
 * `Client` is a mapped type, so `client.get` has no declaration for the
 * bodyless check to read. The language service names the factory's property
 * `get: makeMethod('get')` when asked at the call site.
 */
import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import * as zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { SidecarClient } from './helpers.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const FIXTURE = path.join(__dirname, '..', '..', 'test', 'fixtures', 'position-rows-mapped-impl');

type Row = [string, number, number, string | null, string | null, number | null, string | null, string | null, number];

describe('position_rows: impl rows for a member with no declaration (carrick#2430)', () => {
  let root: string;
  let rows: Row[];
  let source: string;

  before(async () => {
    root = fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-mapped-impl-')));
    const repo = path.join(root, 'repo');
    fs.cpSync(path.join(FIXTURE, 'repo'), repo, { recursive: true });
    const main = path.join(repo, 'src', 'main.ts');
    source = fs.readFileSync(main, 'utf8');
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
      const out = path.join(root, 'store', 'rows.jsonl.gz');
      const answer = await client.send<{ status: string; errors?: string[] }>(
        { action: 'position_rows', request_id: 'rows', files: [main], scan_root: repo, out_path: out, service: 'svc' },
        60_000
      );
      assert.strictEqual(answer.status, 'success', JSON.stringify(answer.errors));
      rows = zlib
        .gunzipSync(fs.readFileSync(out))
        .toString('utf8')
        .trimEnd()
        .split('\n')
        .slice(1)
        .flatMap((line) => (JSON.parse(line) as { r: Row[] }).r);
    } finally {
      await client.stop();
    }
  });

  after(() => {
    fs.rmSync(root, { recursive: true, force: true });
  });

  const implsAt = (text: string): unknown[][] =>
    rows
      .filter((r) => r[0] === 'impl' && source.slice(r[1], r[2]) === text)
      .map((r) => [r[3], r[4], r[5], r[6], r[8]]);

  it('names the factory property for the call and the member of a mapped-type method', () => {
    assert.deepStrictEqual(implsAt("client.get('/items')"), [['get', 'src/client.ts', 19, 'svc', 0]]);
    assert.deepStrictEqual(implsAt('client.get'), [['get', 'src/client.ts', 19, 'svc', 0]]);
  });

  it('answers each member from its own use site', () => {
    assert.deepStrictEqual(implsAt("client.post('/items')"), [['post', 'src/client.ts', 20, 'svc', 0]]);
  });
});
