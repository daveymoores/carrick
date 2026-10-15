import anonymous from "./anonymous";
import satisfied from "./satisfied";
import bound from "./bound";
import { jobs } from "./listed";
import { tasks } from "./renamed";
import { scheduled } from "./declared";

// Each surface reached through the import the module gives it, and its member
// called by name.
export async function dispatch(request: Request): Promise<string> {
  await anonymous.fetch(request);
  await satisfied.fetch(request);
  await bound.fetch(request);
  tasks.drain(1);
  scheduled.run("nightly");
  return jobs.run("daily");
}
