import { transport } from "@fixture/client";

/** A request this service writes itself, as a request spec. */
export async function pingDirect() {
  return transport.get({ url: "/api/v1/ping" });
}
