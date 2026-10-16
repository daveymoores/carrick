import { sendOp } from "./send-op.js";

export async function startAlpha(): Promise<void> {
  await sendOp("alpha", {});
}
