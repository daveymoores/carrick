import { Box, CachedTransport, HttpTransport, Overloaded, type MappedClient } from "./transports";

declare const http: HttpTransport;
declare const cached: CachedTransport;
declare const mapped: MappedClient;
declare const overloaded: Overloaded;
declare const text: Box<string>;
declare const count: Box<number>;

export function run(pick: boolean, options: { client?: HttpTransport }) {
  const a = (pick ? http : cached).get("/a");
  const b = (options.client ?? http).get("/b");
  const c = overloaded.get("/c");
  const d = (pick ? text : count).read();
  const e = (pick ? mapped : http).get("/e");
  return [a, b, c, d, e];
}
