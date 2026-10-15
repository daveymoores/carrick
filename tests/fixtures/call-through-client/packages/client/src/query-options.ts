import { getRun } from "./requests";
import type { RequestOptions } from "./transport";

/** The options a query hook is spread: the key, and the request to make. */
export const getRunOptions = (options: RequestOptions) => ({
  queryKey: ["run", options.path],
  queryFn: () => getRun(options),
});
