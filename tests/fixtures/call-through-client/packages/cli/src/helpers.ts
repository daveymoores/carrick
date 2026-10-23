import { getRun, getStatus, listRuns } from "@fixture/client";
import { pingDirect } from "./local-fetch";

/** Asks for one run and for every run. */
export async function loadPair(id: string) {
  return [await getRun({ path: { id } }), await listRuns()];
}

/** Pings through a request this service writes itself. */
export async function localPing() {
  return pingDirect();
}

/** Asks for the status. */
export async function statusNow() {
  return getStatus();
}

/** Hands back a function that asks for the status; it asks nothing itself. */
export function statusReader() {
  const read = () => getStatus();
  return read;
}
