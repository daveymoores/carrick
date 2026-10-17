export function remote(): Promise<Response> {
  return fetch("/remote");
}
