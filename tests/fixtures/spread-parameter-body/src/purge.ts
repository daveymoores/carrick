import { sendOp } from "./send-op.js";

export async function purgeEverything(id: string, flag: boolean): Promise<void> {
  await sendOp("purge", {});
  await sendOp("purge", { id });
  await sendOp("purge", { op: "evict" });
  await sendOp("purge", flag ? { id: "1" } : {});
}
