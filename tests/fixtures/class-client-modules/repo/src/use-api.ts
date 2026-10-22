import { api } from "./api-instance";

export function loadUsers(): Promise<Response> {
  return api.users();
}
