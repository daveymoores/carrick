import { makeClient } from "./lib/apiClient";
import { config } from "./config";

const ordersClient = makeClient(config.ordersApiUrl);

// The console renders a single "latest event" card — typed as a scalar, but
// the producer returns TimelineEvent[]. Array-vs-scalar mismatch.
export interface TimelineEntry {
  at: string;
  status: string;
  note?: string;
}

export async function loadTimeline(orderId: string): Promise<TimelineEntry> {
  return ordersClient.get<TimelineEntry>(`/orders/${orderId}/timeline`);
}
