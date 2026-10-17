import { api } from "../api";

export function saveB(): Promise<Response> {
  return api("/b", { method: "POST" });
}
