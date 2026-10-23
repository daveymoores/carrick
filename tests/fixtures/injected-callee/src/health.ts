// 5. A parameter's field invoked inside a function written in the body and
// handed to the code that runs it (as a tracer runs its callback), the
// field filled by a caller in another module (`health-check.ts`).
export interface Probe {
  send: typeof fetch;
}

export function ping(path: string, probe: Probe): Promise<Response> {
  return Promise.resolve().then(() => probe.send(path, { method: "HEAD" }));
}
