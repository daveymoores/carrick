// 1. A destructured parameter whose default is the platform's `fetch`,
// called with nothing in its place.
export async function postEvent(body: string, { send = fetch }: { send?: typeof fetch } = {}): Promise<Response> {
  return send("/api/events", { method: "POST", body });
}

export function recordEvent(): Promise<Response> {
  return postEvent("{}");
}
