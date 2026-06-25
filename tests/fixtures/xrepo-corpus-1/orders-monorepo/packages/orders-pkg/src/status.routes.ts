import type { StatusResponse } from './types';

// Trap: constructor-carried prefix, mounted with no path (mount_path "").
// The prefix "/api/v1" is baked into the Router constructor options, so when
// this router is mounted in server.ts it is mounted at "" (no additional path).
// The scanner must honour the constructor-level prefix even when mount_path is "".
//
// Expected resolved endpoint: GET /api/v1/status

// Minimal Router shim — constructor accepts { prefix } and carries it forward.
// This is a deliberate structural pattern (not a real framework router); it lets
// the scanner exercise the "prefix in constructor, no mount path" code path.
class Router {
  readonly prefix: string;
  private _routes: Array<{ method: string; path: string; handler: Function }> = [];

  constructor(opts: { prefix: string }) {
    this.prefix = opts.prefix;
  }

  get<TReply>(path: string, handler: (req: unknown, res: unknown) => TReply): this {
    this._routes.push({ method: 'GET', path, handler });
    return this;
  }

  routes() {
    return this._routes;
  }
}

// The prefix is baked in; the mount path in server.ts is "" (empty string).
const statusRouter = new Router({ prefix: '/api/v1' });

// GET /api/v1/status  — producer-only (no consumer in corpus)
statusRouter.get<StatusResponse>('/status', async (_req, _res): Promise<StatusResponse> => {
  return { status: 'ok', version: '1.0.0', uptime: process.uptime() };
});

export { statusRouter };
