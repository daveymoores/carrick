import { createClient } from "./api-client";
import { loadPair, localPing, statusNow, statusReader, statusViaReader } from "./helpers";

/** The model states the request at the call written around the client's. */
export async function showGatheredStatus() {
  const client = createClient();
  const [status] = await Promise.all([
    client.getStatus(),
  ]);
  return status;
}

/** Two functions that reach one operation, called on one line. */
export async function showStatusTwice() {
  const client = createClient();
  return Promise.all([client.getStatus(), statusNow()]);
}

/** The function called reaches two operations of the other service. */
export async function showPair(id: string) {
  const pair = await loadPair(id);
  return pair;
}

/** The function called requests a route of this service's own. */
export async function showLocalPing() {
  const reply = await localPing();
  return reply;
}

/** The function called builds a reader and hands it back unrun. */
export function makeReader() {
  const read = statusReader();
  return read;
}

/** The function called writes a reader and runs it. */
export async function showViaReader() {
  const status = await statusViaReader();
  return status;
}
