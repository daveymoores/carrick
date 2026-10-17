import { Queue } from "@fixture/queue";

export const emails = new Queue("emails");

let lazy: Queue | null = null;

export function getQueue() {
  if (!lazy) {
    lazy = new Queue("lazy");
  }
  return lazy;
}
