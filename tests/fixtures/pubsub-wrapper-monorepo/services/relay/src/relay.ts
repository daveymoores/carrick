// Relay service: the SUBSCRIBER side of all three wrapper shapes. Every
// handler receives its payload through a generic binding — no payload type is
// ever written as a named annotation at the subscribe site.
import {
  bus,
  channel,
  jobCatalog,
  QueueWorker,
} from "@fixture/contracts";

// Shape 1 consumer: destructured, unannotated handler param on the typed bus.
export function watchArchives(): void {
  bus.on("itemArchived", ({ time, item }) => {
    console.log(time.toISOString(), item.id, item.status, item.error?.code);
  });
}

// Shape 2 consumer: the jobs-map handler destructures the payload out of the
// worker envelope; its type is InferSchema<catalog["records.reindex"]["schema"]>.
export const worker = new QueueWorker({
  catalog: jobCatalog,
  jobs: {
    "records.reindex": async ({ payload }) => {
      console.log(payload.resourceId, payload.mode);
    },
  },
});

// Shape 3 consumer: this service declares its own handle for the shared
// channel id; the payload type is a declaration-site type argument.
const approvals = channel<{ approved: boolean; reviewer: string }>({
  id: "approval",
});

export function watchApprovals(): void {
  approvals.on((decision) => {
    console.log(decision.approved, decision.reviewer);
  });
}
