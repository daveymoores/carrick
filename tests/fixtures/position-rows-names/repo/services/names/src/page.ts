import { listUsers } from "./api";

export async function load(): Promise<void> {
  await listUsers();
}
