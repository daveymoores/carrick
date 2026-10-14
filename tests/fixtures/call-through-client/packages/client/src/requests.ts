import { transport, type RequestOptions } from "./transport";

/** A request written as a spec: the verb is the member, the path is `url`. */
export const getRun = (options: RequestOptions) =>
  (options.client ?? transport).get({ url: "/api/v1/runs/{id}", ...options });

export const listRuns = (options: RequestOptions = {}) =>
  (options.client ?? transport).get({ url: "/api/v1/runs", ...options });

export const saveRun = (options: RequestOptions) =>
  (options.client ?? transport).put({ url: "/api/v1/runs/{id}", ...options });

export const getToken = (options: RequestOptions = {}) =>
  (options.client ?? transport).get({ url: "/api/v1/token", ...options });

export const recordAudit = (options: RequestOptions) =>
  (options.client ?? transport).post({ url: "/api/v1/audit", ...options });

export const checkHealth = (options: RequestOptions = {}) =>
  (options.client ?? transport).get({ url: "/health", ...options });

export const getStatus = (options: RequestOptions = {}) =>
  (options.client ?? transport).get({ url: "/api/v1/status", ...options });

/** A request written as a `fetch` with a literal path. */
export async function ping() {
  const response = await fetch("/api/v1/ping");
  return { data: await response.json() };
}

/** A request whose whole URL is its parameters: no row can state its path. */
export async function send(base: string, path: string) {
  const response = await fetch(`${base}${path}`);
  return { data: await response.json() };
}

/** The API serves this path under another verb. */
export const archiveRun = (options: RequestOptions) =>
  (options.client ?? transport).post({ url: "/api/v1/runs/{id}/archive", ...options });
