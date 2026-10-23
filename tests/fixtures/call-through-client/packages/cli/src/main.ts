import { showRun } from "./commands";

/** The entry point: calls a command, which calls the client. */
export async function main(argv: string[]) {
  return showRun(argv[2] ?? "latest");
}
