import { bus } from "@fixture/bus";
import { TOPICS, PREFIX, BROKEN } from "./barrel";
import { ALIASED } from "@lib/aliased";
import { emails, getQueue } from "./queue";
import { LEGACY } from "./legacy.js";

export async function send() {
  bus.publish(TOPICS.orders, {});
  bus.publish(PREFIX, {});
  bus.publish(BROKEN, {});
  bus.publish(ALIASED.send, {});
  bus.publish(LEGACY.send, {});
  await emails.add("welcome", {});
  const held = getQueue();
  await held.add("later", {});
}
