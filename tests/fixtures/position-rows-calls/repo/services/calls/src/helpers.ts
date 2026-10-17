export function ping(): Promise<Response> {
  return fetch("/ping");
}
