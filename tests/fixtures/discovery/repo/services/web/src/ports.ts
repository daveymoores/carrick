export interface Port {
  send(name: string, body: unknown): Promise<void>;
}

export function notifyAll(port: Port): Promise<void> {
  return port.send("all", {});
}

export function withRequester(req: (path: string) => Promise<unknown>): Promise<unknown> {
  return req("/injected");
}
