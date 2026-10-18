import type { Jobs } from "./types";

// The declared form, through a type check: `export const` with `satisfies`.
export const scheduled = {
  run(name: string): string {
    return name.toUpperCase();
  },
} satisfies Jobs;
