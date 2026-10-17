import type { Entry } from "./entry";
import defaults from "./defaults.json";

export type Defaults = typeof defaults;

export interface Ledger {
  entries: Entry[];
  closed: boolean;
  defaults: Defaults;
}
