export function request(path: string, init?: RequestInit): Promise<Response> {
  return fetch(path, init);
}
