import { sendOp } from "./send-op.js";

function buildArgs(): object {
  return { id: "1" };
}

export async function purgeUnread(list: [string, object]): Promise<void> {
  const extra = {};
  await sendOp("purge", extra);
  await sendOp("purge", buildArgs());
  await sendOp(...list);
  await sendOp("purge");
}
