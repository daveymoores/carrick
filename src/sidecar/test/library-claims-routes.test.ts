/**
 * carrick#2474 R0: route fields on `verify_library_claims` (contract
 * amendment of 2026-10-22 on carrick#2434). Each field is checked against the
 * package's own declarations, one valid claim and one invalid claim per field:
 * - `mounts` on a scope: the instance handed there is served under the
 *   scope's name (`mount(prefix, child)`);
 * - `runs` with `yields` on a scope: the member runs a function and hands it
 *   a scoped instance (`register(fn, { prefix })`);
 * - `request` on a receive op: where the handler's request holds its path,
 *   URL and method.
 *
 * The package is invented and hand-written from the general shape of a
 * server package (a maker returning an app whose verb members take
 * `(path, handler)`, a sub-router maker, a mount member, a plugin member,
 * a request type with path, URL and method members), never copied from a
 * real one. Claims with none of these fields are pinned unchanged by
 * `library-claims.test.ts` and `library-claims-v2.test.ts`.
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

// A server package: calling the export makes an app, `router()` a sub-router.
// `mount(prefix, child)` serves a router under a prefix; `register(fn, opts)`
// runs a plugin with a scoped app, and its return type says nothing.
const SERVER = `export interface Request {
  path: string;
  method: string;
  raw: { url: string };
}
export interface Reply {
  send(body: unknown): void;
}
export type Handler = (request: Request, reply: Reply) => void;
export interface Router {
  get(path: string, handler: Handler): this;
  post(path: string, handler: Handler): this;
  mount(prefix: string, child: Router): this;
}
export interface App extends Router {
  register(fn: (app: App) => void, options: { prefix?: string; retries?: number }): unknown;
}
export interface Server {
  (): App;
  router(): Router;
}
declare const server: Server;
export default server;
`;

function writeTree(root: string, files: Record<string, string>): void {
  for (const [rel, text] of Object.entries(files)) {
    const file = path.join(root, rel);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
  }
}

type Claim = Record<string, unknown> & { kind: string };
interface Check {
  claim_id: string;
  package: string;
  export: string;
  role: string;
  receiver: string;
  claim: Claim;
}
interface Response {
  status: string;
  verdicts?: Array<{ verdict: string; reason?: string }>;
  errors?: string[];
}

const P = 'fixture-server';
let sequence = 0;
const check = (receiver: string, claim: Claim): Check => ({
  claim_id: `r${sequence++}`,
  package: P,
  export: 'default',
  role: 'server_framework',
  receiver,
  claim,
});
const makers = [
  check('export', { kind: 'make', form: 'call', member: null }),
  check('export', { kind: 'make', form: 'call', member: 'router' }),
];
const onApp = (claim: Claim) => check('instance:()', { ...claim, of: 'instance:()' });
const onRouter = (claim: Claim) => check('instance:router', { ...claim, of: 'instance:router' });
const scope = (member: string, parts: Record<string, unknown>): Claim => ({ kind: 'scope', member, ...parts });
const route = (member: string, parts: Record<string, unknown>): Claim => ({
  kind: 'op',
  op: 'receive',
  member,
  name: { arg: 0 },
  handler: { arg: 1 },
  method: member.toUpperCase(),
  ...parts,
});

function verdicts(response: Response): string[] {
  assert.strictEqual(response.status, 'success', JSON.stringify(response.errors));
  return response.verdicts!.map(r => (r.verdict === 'verified' ? 'verified' : `${r.verdict} ${r.reason}`));
}

describe('verify_library_claims: route fields (carrick#2474 R0)', () => {
  let root: string;
  let client: SidecarClient;
  let requestId = 0;

  // The makers lead every request; their two verdicts are dropped.
  const send = async (checks: Check[]) =>
    verdicts(
      await client.send<Response>(
        { request_id: `routes-${requestId++}`, action: 'verify_library_claims', from_dir: root, checks: [...makers, ...checks] },
        60_000
      )
    ).slice(makers.length);

  before(async () => {
    root = fs.mkdtempSync(path.join(os.tmpdir(), 'carrick-claims-routes-'));
    writeTree(root, {
      'tsconfig.json': TSCONFIG,
      'package.json': JSON.stringify({ name: 'service' }),
      'src/index.ts': 'export const service = 1;\n',
      'node_modules/fixture-server/package.json': JSON.stringify({ name: P, version: '1.0.0', types: 'index.d.ts' }),
      'node_modules/fixture-server/index.d.ts': SERVER,
    });
    client = new SidecarClient();
    await client.start();
    const ready = await client.send<{ status: string }>({ request_id: 'init', action: 'init', repo_root: root });
    assert.strictEqual(ready.status, 'ready');
  });

  after(async () => {
    await client.stop();
    fs.rmSync(root, { recursive: true, force: true });
  });

  it('the makers and a plain route verify, as before', async () => {
    const response = await client.send<Response>(
      { request_id: 'routes-base', action: 'verify_library_claims', from_dir: root, checks: [...makers, onApp(route('get', {}))] },
      60_000
    );
    assert.deepStrictEqual(verdicts(response), ['verified', 'verified', 'verified']);
  });

  it('mounts: the instance handed there is served under the scope name', async () => {
    const mount = (parts: Record<string, unknown>) => scope('mount', { name: { arg: 0 }, ...parts });
    assert.deepStrictEqual(
      await send([
        onApp(mount({ mounts: { arg: 1 } })),
        onRouter(mount({ mounts: { arg: 1 } })),
        // Invalid: only a function fits the handler parameter.
        onApp(scope('get', { name: { arg: 0 }, mounts: { arg: 1 } })),
        // Invalid: no argument there.
        onApp(mount({ mounts: { arg: 4 } })),
      ]),
      ['verified', 'verified', 'failed mounts_not_instance', 'failed param_missing']
    );
  });

  it('runs with yields: the member runs a plugin and hands it a scoped app, whatever it returns', async () => {
    // `retries` is the options' other key: labelled, so the prefix is the only name.
    const register = (parts: Record<string, unknown>) =>
      onApp(scope('register', { name: { arg: 1, key: 'prefix' }, key_labels: { retries: 'not_name' }, ...parts }));
    const yields = { param: 0, export: 'App', side: 'server' };
    assert.deepStrictEqual(
      await send([
        register({ runs: { arg: 0 }, yields }),
        // Invalid: the key named holds a number, not a function.
        register({ runs: { arg: 1, key: 'retries' }, yields }),
        // Invalid: the plugin takes no parameter there.
        register({ runs: { arg: 0 }, yields: { ...yields, param: 2 } }),
      ]),
      ['verified', 'failed runs_not_function', 'failed yields_not_export']
    );
  });

  it('a scope that binds through runs or mounts is never read through its return', async () => {
    // The plugin member's return says nothing: without `runs` it is unresolved, as before.
    const prefix = { name: { arg: 1, key: 'prefix' }, key_labels: { retries: 'not_name' } };
    assert.deepStrictEqual(
      await send([
        onApp(scope('register', prefix)),
        onApp(scope('mount', { name: { arg: 0 }, mounts: { arg: 1 } })),
        // What a mount scope returns is not read as a receiver.
        check('instance:()>scope:mount', route('get', { of: 'instance:()>scope:mount' })),
      ]),
      ['unchecked scope_unresolved', 'verified', 'unchecked scope_unresolved']
    );
  });

  it('runs, yields and mounts pair as the contract says', async () => {
    const yields = { param: 0, export: 'App', side: 'server' };
    const register = (parts: Record<string, unknown>) =>
      onApp(scope('register', { name: { arg: 1, key: 'prefix' }, key_labels: { retries: 'not_name' }, ...parts }));
    assert.deepStrictEqual(
      await send([
        register({ runs: { arg: 0 } }),
        register({ yields }),
        onApp(scope('mount', { name: { arg: 0 }, mounts: { arg: 1 }, runs: { arg: 1 }, yields })),
      ]),
      ['unchecked claim_invalid', 'unchecked claim_invalid', 'unchecked claim_invalid']
    );
  });

  it('request: each member path resolves on the handler’s request parameter', async () => {
    assert.deepStrictEqual(
      await send([
        onApp(route('get', { request: { param: 0, path: ['path'], url: ['raw', 'url'], method: ['method'] } })),
        onRouter(route('post', { request: { param: 0, url: ['raw', 'url'] } })),
        // Invalid: the request declares no such member.
        onApp(route('get', { request: { param: 0, path: ['pathname'] } })),
        // Invalid: the second step does not resolve.
        onApp(route('get', { request: { param: 0, url: ['raw', 'href'] } })),
        // Invalid: the handler's parameter there is the reply.
        onApp(route('get', { request: { param: 1, path: ['path'] } })),
      ]),
      ['verified', 'verified', 'failed request_member_missing', 'failed request_member_missing', 'failed request_member_missing']
    );
  });

  it('request is read on a receive op with a handler that names a path or a URL', async () => {
    assert.deepStrictEqual(
      await send([
        onApp(route('get', { request: { param: 0, method: ['method'] } })),
        onApp(route('get', { request: { param: 0, path: [] } })),
        onApp(route('get', { handler: undefined, request: { param: 0, path: ['path'] } })),
      ]),
      ['unchecked claim_invalid', 'unchecked claim_invalid', 'unchecked claim_invalid']
    );
  });

  it('refuses a request whose route field is outside the shape', async () => {
    const shapes: Claim[] = [
      scope('mount', { name: { arg: 0 }, mounts: { arg: -1 } }),
      scope('register', { name: { arg: 1, key: 'prefix' }, runs: { bound: 'maker', arg: 0 } }),
      route('get', { request: { param: 0, path: [''] } }),
      route('get', { request: { param: 0, path: ['path'], query: ['q'] } }),
    ];
    for (const claim of shapes) {
      const response = await client.send<Response>(
        { request_id: `routes-${requestId++}`, action: 'verify_library_claims', from_dir: root, checks: [onApp(claim)] },
        60_000
      );
      assert.strictEqual(response.status, 'error', JSON.stringify(claim));
    }
  });
});
