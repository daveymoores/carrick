import { client } from "rpc-transport";

export function bind<T>(local: T, name: string, options: { timeoutMs: number }) {
  const timeout = options.timeoutMs;
  return async (...rest: unknown[]) => {
    const send = () => client.send(name, rest, timeout);
    return send();
  };
}

export function endpoint(path: string) {
  return (id: string) => client.send(path, id);
}

export function eager(name: string) {
  void fetch("/api/warm");
  return () => name;
}
