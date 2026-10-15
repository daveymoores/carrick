export type Handler = (input?: unknown) => unknown;

const table = new Map<string, Handler>();

export function register(handler: Handler, name: string): void {
  table.set(name, handler);
}

export function registerNamed(name: string, handler: Handler): void {
  table.set(name, handler);
}

export async function registerAsync(handler: Handler, name: string): Promise<void> {
  table.set(name, handler);
}

export async function dispatch(name: string, input?: unknown): Promise<unknown> {
  return table.get(name)!(input);
}
