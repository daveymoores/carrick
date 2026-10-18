import { depClient } from "dep";
import { makeMapped, makePlain, type MappedClient, type PlainClient } from "./client";

const mapped = makeMapped();
const plain = makePlain();

export function run(options: { mapped?: MappedClient; plain?: PlainClient }) {
  const a = (options.mapped ?? mapped).get("/a");
  const b = (options.plain ?? plain).get("/b");
  const c = depClient.post("/c");
  const d = mapped["post"]("/d");
  return [a, b, c, d];
}
