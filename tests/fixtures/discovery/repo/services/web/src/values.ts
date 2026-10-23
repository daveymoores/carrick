import { request } from "fixture-http";

// A destructured parameter whose default is the platform's `fetch`, called
// with nothing in its place.
export async function postEvent(body: string, { send = fetch }: { send?: typeof fetch } = {}): Promise<Response> {
  return send("/api/events", { method: "POST", body });
}

export function recordEvent(): Promise<Response> {
  return postEvent("{}");
}

// A field of a parameter, filled two calls up by the composition root.
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

// Two callers handing two different labelled exports.
export function load(path: string, get: (url: string) => Promise<unknown>): Promise<unknown> {
  return get(path);
}

export function viaFetch(): Promise<unknown> {
  return load("/api/a", fetch);
}

export function viaRequest(): Promise<unknown> {
  return load("/api/b", request);
}
