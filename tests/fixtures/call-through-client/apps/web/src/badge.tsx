import { listRuns } from "@fixture/client";

/** Lists the runs when it renders. */
export function RunBadge() {
  void listRuns();
  return null;
}
