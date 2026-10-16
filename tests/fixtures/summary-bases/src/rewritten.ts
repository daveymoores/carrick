import { apiBase } from "./config";
import { getThings } from "./things";

export async function loadThings(): Promise<string | undefined> {
  await getThings();
  return apiBase;
}
