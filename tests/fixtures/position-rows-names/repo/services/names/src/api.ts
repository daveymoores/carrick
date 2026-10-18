import { API_BASE } from "./config";

export async function listUsers(): Promise<Response> {
  return fetch(`${API_BASE}/users`);
}
