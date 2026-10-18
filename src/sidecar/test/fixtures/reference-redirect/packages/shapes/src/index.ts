import type { ParsedPath } from "node:path";
import type { Money } from "@shapes-internal/money";

export type { Money } from "@shapes-internal/money";

export interface Order {
  id: string;
  total: Money;
  file: ParsedPath;
}
