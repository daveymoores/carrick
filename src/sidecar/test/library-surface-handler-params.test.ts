/**
 * carrick#2674 S3: `list_library_surface` lists the members a route handler's
 * own parameters declare, so a picker can name `request.body` on a receive
 * op's `request` field. They count against the per-package cap and are listed
 * before any signature, so a large framework does not lose them.
 *
 * Every package below is invented and its declarations are hand-written.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { SidecarClient } from './helpers.js';

const TSCONFIG = JSON.stringify({
  compilerOptions: {
    target: 'es2020',
    module: 'commonjs',
    moduleResolution: 'node',
    strict: true,
    esModuleInterop: true,
    skipLibCheck: true,
    types: [],
  },
  include: ['src/**/*.ts'],
});

const ROUTES = `export interface Request {
  body: unknown;
  params: Record<string, string>;
  headers: { host: string; accept?: string };
  raw: { url: string; method: string };
  get(name: string): string | undefined;
}
export interface Reply {
  send(body: unknown): void;
  status(code: number): this;
}
export type Handler = (request: Request, reply: Reply) => void;
export interface Router {
  get(path: string, handler: Handler): this;
  get(path: string, options: { schema?: object }, handler: Handler): this;
  route(options: { url: string; handler: Handler }): this;
  listen(port: number): void;
  on(event: string, listener: (payload: unknown) => void): this;
}
export declare function router(): Router;
`;

// Eight verbs that each take the same handler, so the handler members repeat
// across members and the cap bites before the signatures are listed.
const VERBS = ['get', 'post', 'put', 'patch', 'delete', 'head', 'options', 'all'];
const BIG = `export interface Req {
  body: unknown;
  query: { page?: string };
  raw: { url: string };
}
export type Handle = (req: Req) => void;
export interface Big {
${VERBS.map(verb => `  ${verb}(path: string, handler: Handle): this;`).join('\n')}
}
export declare function big(): Big;
`;

interface HandlerMember {
  path: string[];
  type: string;
  optional: boolean;
  function: boolean;
}
interface HandlerParam {
  param: number;
  name: string;
  type: string;
  members: HandlerMember[];
}
interface Handler {
  arg: number;
  key?: string;
  params: HandlerParam[];
}
interface Member {
  name: string;
  handlers?: Handler[];
  signatures: Array<{ params: unknown[] }>;
}
interface Surface {
  package: string;
  truncated: number;
  exports: Array<{ export: string; receivers: Array<{ receiver: string; members: Member[] }> }>;
}

describe('list_library_surface: handler parameter members (carrick#2674 S3)', () => {
  let client: SidecarClient;
  let root: string;
  let requestId = 0;

  async function surface(packages: string[], extra: Record<string, unknown> = {}): Promise<Surface[]> {
    const response = await client.send<{ status: string; surfaces: Surface[]; errors?: string[] }>(
      { request_id: `s-${requestId++}`, action: 'list_library_surface', from_dir: root, packages, ...extra },
      60_000
    );
    assert.strictEqual(response.status, 'success', JSON.stringify(response.errors));
    return response.surfaces;
  }

  const membersOf = (listed: Surface): Member[] =>
    listed.exports.flatMap(e => e.receivers.flatMap(r => r.members));

  before(async () => {
    root = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-surface-handlers-'));
    const files: Record<string, string> = {
      'tsconfig.json': TSCONFIG,
      'package.json': JSON.stringify({ name: 'svc' }),
      'src/index.ts': 'export const service = 1;\n',
    };
    for (const [name, text] of [['fixture-routes', ROUTES], ['fixture-big', BIG]]) {
      files[`node_modules/${name}/package.json`] = JSON.stringify({ name, version: '1.0.0', types: 'index.d.ts' });
      files[`node_modules/${name}/index.d.ts`] = text;
    }
    for (const [rel, text] of Object.entries(files)) {
      const file = path.join(root, rel);
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, text);
    }
    client = new SidecarClient();
    await client.start();
    const init = await client.send<{ status: string }>({ request_id: 'init', action: 'init', repo_root: root });
    assert.strictEqual(init.status, 'ready');
  });

  after(async () => {
    await client.stop();
    fs.rmSync(root, { recursive: true, force: true });
  });

  it('lists the members of a handler`s parameters, positional and keyed, to two levels', async () => {
    const [routes] = await surface(['fixture-routes']);
    const byName = new Map(membersOf(routes).map(m => [m.name, m]));
    const get = byName.get('get')!;
    // The overloads put the handler at argument 1 and at argument 2: one each.
    assert.deepStrictEqual(get.handlers!.map(h => h.arg), [1, 2]);
    assert.deepStrictEqual(get.handlers![1].params, get.handlers![0].params);
    const [handler] = get.handlers!;
    assert.strictEqual(handler.arg, 1);
    assert.strictEqual(handler.key, undefined);
    const [request, reply] = handler.params;
    assert.deepStrictEqual([request.param, request.name, request.type], [0, 'request', 'Request']);
    assert.deepStrictEqual(
      request.members.map(m => [m.path.join('.'), m.type, m.optional, m.function]),
      [
        ['body', 'unknown', false, false],
        ['get', '(name: string) => string | undefined', false, true],
        ['headers', '{ host: string; accept?: string | undefined; }', false, false],
        ['headers.accept', 'string | undefined', true, false],
        ['headers.host', 'string', false, false],
        ['params', 'Record<string, string>', false, false],
        ['raw', '{ url: string; method: string; }', false, false],
        ['raw.method', 'string', false, false],
        ['raw.url', 'string', false, false],
      ]
    );
    assert.deepStrictEqual([reply.param, reply.name], [1, 'reply']);
    assert.deepStrictEqual(reply.members.map(m => m.path.join('.')), ['send', 'status']);

    // A handler held in a key of the options object is listed at that key.
    const route = byName.get('route')!;
    assert.deepStrictEqual(route.handlers!.map(h => [h.arg, h.key]), [[0, 'handler']]);
    assert.strictEqual(route.handlers![0].params[0].name, 'request');

    // A member that takes no function, and a callback whose parameter declares no member, list none.
    assert.strictEqual(byName.get('listen')!.handlers, undefined);
    assert.strictEqual(byName.get('on')!.handlers, undefined);
  });

  it('counts the members against the cap and lists them before any signature', async () => {
    const [full] = await surface(['fixture-big']);
    const fullMembers = membersOf(full);
    assert.deepStrictEqual(fullMembers.map(m => m.name), [...VERBS].sort());
    assert.strictEqual(full.truncated, 0);
    const handlerEntries = fullMembers.reduce(
      (n, m) => n + m.handlers!.reduce((k, h) => k + h.params.reduce((j, p) => j + 1 + p.members.length, 0), 0),
      0
    );
    // Entries before the signatures: the export, its receivers, each member name, each handler entry.
    const receivers = full.exports.reduce((n, e) => n + e.receivers.length, 0);
    const before = full.exports.length + receivers + fullMembers.length + handlerEntries;
    const [capped] = await surface(['fixture-big'], { max_entries: before });
    assert.deepStrictEqual(
      membersOf(capped).map(m => [m.name, m.handlers]),
      fullMembers.map(m => [m.name, m.handlers])
    );
    // The cap cut the signatures, and counted them.
    assert.ok(capped.truncated > 0, 'nothing was cut');
    assert.ok(membersOf(capped).every(m => m.signatures.length === 0));
    // One entry fewer loses the last handler member, not a member name.
    const [short] = await surface(['fixture-big'], { max_entries: before - 1 });
    assert.deepStrictEqual(membersOf(short).map(m => m.name), [...VERBS].sort());
    const shortEntries = membersOf(short).reduce(
      (n, m) => n + m.handlers!.reduce((k, h) => k + h.params.reduce((j, p) => j + 1 + p.members.length, 0), 0),
      0
    );
    assert.strictEqual(shortEntries, handlerEntries - 1);
  });
});
