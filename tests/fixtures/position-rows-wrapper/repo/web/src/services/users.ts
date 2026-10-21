import { apiGet, apiPost, apiPut, apiDelete } from "../lib/api";

export interface User { id: string; name: string }

export function listUsers() {
  return apiGet<User[]>("/users");
}

export function getUser(id: string) {
  return apiGet<User>(`/users/${id}`);
}

export function createUser(input: { name: string }) {
  return apiPost<User>("/users", input);
}

export function renameUser(id: string, name: string) {
  return apiPut<User>(`/users/${id}`, { name });
}

export function removeUser(id: string) {
  return apiDelete(`/users/${id}`);
}
