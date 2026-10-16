import { sendOp } from "./send-op.js";

export async function startBeta(): Promise<void> {
  await sendOp("beta", {});
}
