export interface Status {
  healthy: boolean;
}

/** What the API sends for a run. */
export interface Run {
  id: string;
  state: string;
}

/** A stale copy: the API stopped sending `savedBy`. */
export interface SavedRun {
  id: string;
  state: string;
  savedBy: string;
}

/** What a caller reads of a run, with the time it was read. */
export interface StampedRun {
  run: Run;
  readAt: number;
}
