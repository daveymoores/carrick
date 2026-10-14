import { createClient } from "@fixture/sdk";

/** No annotation: the body's only statement returns another factory's call. */
export function getClient() {
  return createClient();
}

/** Three functions deep: `sharedClient`, `getClient`, `createClient`. */
export const sharedClient = () => getClient();

/** Four functions deep, which is one more than is read. */
export const distantClient = () => sharedClient();
