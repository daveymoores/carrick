import { Worker } from "bullmq";
import { DispatchJob } from "./types/fulfillment";
import { publishStatusChanged } from "./status.publisher";

// BullMQ worker: the queue name is the contract topic.
export const dispatchWorker = new Worker("shipments.dispatch", async (job) => {
  const req = job.data as DispatchJob;
  await dispatchShipment(req);
  await publishStatusChanged({
    orderId: req.orderId,
    status: "dispatched",
    occurredAt: new Date().toISOString(),
  });
});

async function dispatchShipment(req: DispatchJob): Promise<void> {
  void req;
}
