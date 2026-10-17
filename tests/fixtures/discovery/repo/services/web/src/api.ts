export function api(path: string, opts?: RequestInit): Promise<Response> {
  return fetch(path, opts);
}

export function v1(path: string): Promise<Response> {
  return api("/v1" + path);
}

export function fromEntry(path: string): Promise<Response> {
  return api(path);
}
