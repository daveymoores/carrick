export interface RunList {
  runs: { id: string; state: string }[];
}

/** What a query-options factory is handed: the request's options. */
export interface RunQueryInput {
  path: { id: string };
}
