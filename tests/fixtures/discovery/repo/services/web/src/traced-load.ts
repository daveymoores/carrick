import { traced } from "./tracer";

export function loadTraced(): Promise<Response> {
  return traced("load", () => fetch("/traced"));
}
