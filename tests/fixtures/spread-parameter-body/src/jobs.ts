import { sendCopied, sendExtra, sendStamped, sendTwice } from "./refused.js";

export async function runJobs(): Promise<void> {
  await sendTwice("twice", {}, {});
  await sendExtra("extra", { extra: {} });
  await sendCopied("copied", {});
  await sendStamped("stamped", {});
}
