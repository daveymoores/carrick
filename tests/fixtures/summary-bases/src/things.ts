import { apiBase } from "./config";

export function getThings(): Promise<Response> {
  return fetch(`${apiBase}/things`);
}

export function ping(): Promise<Response> {
  return fetch(`${API_ORIGIN}/ping`);
}
