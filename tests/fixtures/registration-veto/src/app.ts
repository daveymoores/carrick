import { check } from "./check";
import { setup } from "./setup";

export function boot() {
  setup();
  check();
}
