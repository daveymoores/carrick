import { api } from "../api";

export function loadC(): Promise<Response> {
  return api("/c");
}
