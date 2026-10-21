import { listUsers, createUser, removeUser, renameUser } from "../services/users";
import { apiGet } from "../lib/api";

export async function loadUserPanel() {
  const users = await listUsers();
  const me = await apiGet<{ id: string }>("/me");
  return { users, me };
}

export async function onCreate(name: string) {
  return createUser({ name });
}

export async function onRename(id: string, name: string) {
  return renameUser(id, name);
}

export async function onRemove(id: string) {
  await removeUser(id);
}
