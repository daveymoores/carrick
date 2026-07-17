// Dispatch service: the PUBLISHER side of all three wrapper shapes. Every
// payload is an inline object literal, a property initializer, or a local
// variable — never a value with a named payload type.
import { bus, channel } from "@fixture/contracts";
import { worker } from "@fixture/relay";

// Shape 1 producer: inline object literal payload on the typed bus.
export function archiveItem(itemId: string): void {
  bus.emit("itemArchived", {
    time: new Date(),
    item: { id: itemId, status: "archived", error: null },
  });
}

// Shape 2 producer: the payload property initializer's type is derived from
// the catalog schema; nothing at this site names it.
export async function requestReindex(resourceId: string): Promise<void> {
  await worker.enqueue({
    id: `reindex-${resourceId}`,
    job: "records.reindex",
    payload: { resourceId, mode: "full" },
  });
}

// Shape 3 producer: this service declares its own handle for the shared
// channel id and sends a locally-typed value.
const approvals = channel<{ approved: boolean; reviewer: string }>({
  id: "approval",
});

export async function approveRun(runId: string): Promise<void> {
  const decision = { approved: true, reviewer: "auto-policy" };
  await approvals.send(runId, decision);
}
