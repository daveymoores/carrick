import { tasksApi } from "./tasks-api";

export async function closeTask(id: string): Promise<void> {
  await tasksApi.setStatus(id, "closed");
}

export async function openTask(id: string): Promise<unknown> {
  return tasksApi.load(id);
}

export async function archiveTask(id: string): Promise<void> {
  await tasksApi.archive(id);
}
