// 2. A field of a parameter, filled two calls up by the composition root.
export interface SyncDeps {
  fetch: typeof fetch;
}

export function sync(deps: SyncDeps): Promise<Response> {
  return deps.fetch("/api/sync", { method: "PUT" });
}

function startSync(deps: SyncDeps): Promise<Response> {
  return sync(deps);
}

export function boot(): Promise<Response> {
  return startSync({ fetch });
}
