import { queryOptions, useQuery } from "@fixture/query";
import { getRun, getRunOptions, getStatus, listRuns, recordAudit } from "@fixture/client";
import { retry, withSpinner } from "./spinner";
import type { RunList, RunQueryInput } from "./models";
declare const vendor: { send(body: unknown): Promise<unknown> };

/** The hook is spread the options a client function returns. */
export function useRun(id: string) {
  return useQuery({ ...getRunOptions({ path: { id } }) });
}

/** The hook is handed one callback, which calls into the client. */
export function useRuns() {
  return useQuery<RunList>({ queryKey: ["runs"], queryFn: () => listRuns() });
}

/** A callback handed to one of the app's own functions. */
export async function auditVisit(entry: unknown) {
  await withSpinner(() => recordAudit({ body: entry }));
}

/** Two callbacks: nothing says which of them makes the request. */
export async function sendWithRetry(body: unknown) {
  await retry(() => vendor.send(body), { onRetry: () => recordAudit({ body }) });
}

/** The hook is handed one callback, and the model states its request. */
export function useStatus() {
  return useQuery({ queryKey: ["status"], queryFn: () => getStatus() });
}

/** Query options built by the query library's helper around one callback. */
export const runQueryOptions = (options: RunQueryInput) => {
  return queryOptions({
    queryKey: ["run", options.path],
    queryFn: () => getRun(options),
  });
};
