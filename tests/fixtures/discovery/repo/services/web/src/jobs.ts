import { Bus } from "./bus";

const bus = new Bus();

export function startJob(id: string): void {
  bus.emit("job.start", { id });
}
