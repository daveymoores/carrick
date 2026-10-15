import type { Jobs } from "./types";

// Shape 3, named: the export list is written before the declaration, and the
// object is wrapped in parentheses and an assertion.
export { jobs };

const jobs = ({
  run(name: string): string {
    return name;
  },
}) as Jobs;
