import { api } from "../api";

export function loadA(): Promise<Response> {
  return api("/a");
}
