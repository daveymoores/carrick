import { Ops } from "./ops";

type Handler = (args: unknown) => unknown;

function saveNote(args: unknown): unknown {
  return args;
}

const handlers = new Map<string, Handler>();
handlers.set(Ops.SAVE, (args) => saveNote(args));
handlers.set(Ops.LOAD, saveNote);

export function handle(name: string, args: unknown): unknown {
  return handlers.get(name)?.(args);
}
