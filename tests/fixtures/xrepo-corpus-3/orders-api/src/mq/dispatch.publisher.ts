import { Queue } from "bullmq";
import { DispatchRequest } from "../types/orders";

// BullMQ: the contract topic is the QUEUE name ("shipments.dispatch"); the
// job name passed to add() is not part of the identity.
const dispatchQueue = new Queue("shipments.dispatch");

export async function scheduleDispatch(req: DispatchRequest): Promise<void> {
  await dispatchQueue.add("dispatch", req);
}
