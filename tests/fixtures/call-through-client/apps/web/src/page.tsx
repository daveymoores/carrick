import { RunBadge } from "./badge";

/** Renders the badge as an element. */
export function Page() {
  return <RunBadge />;
}

/** Calls the component as a function. */
export function renderBadge() {
  return RunBadge();
}
