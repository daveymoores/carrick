import { api, east, items } from "./clients";

export async function load() {
  await api.users();
  await items.list();
  await east.ping();
}
