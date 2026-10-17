import { connect } from "fixture-broker";

const broker = connect("amqp://jobs.internal");

function handleResize(payload: unknown): void {
  console.log(payload);
}

function handleThumbnail(payload: unknown): void {
  console.log(payload);
}

function handleArchive(payload: unknown): void {
  console.log(payload);
}

const jobs: Record<string, (payload: unknown) => void> = {
  "image.resize": handleResize,
  "image.thumbnail": handleThumbnail,
  "image.archive": handleArchive,
};

export function registerJobs(): void {
  for (const [name, handler] of Object.entries(jobs)) {
    broker.subscribe(name, handler);
  }
}
