// 6. Two callers handing two different values: the platform's `fetch`, and
// a function that sends nothing.
export function load(path: string, get: (url: string) => Promise<unknown>): Promise<unknown> {
  return get(path);
}

async function offline(url: string): Promise<unknown> {
  return { url };
}

export function live(): Promise<unknown> {
  return load("/api/live", fetch);
}

export function replay(): Promise<unknown> {
  return load("/api/replay", offline);
}
