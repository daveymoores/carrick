import { Hosts } from "./hosts";

export function listHosts(): Promise<Response> {
  return Hosts.local().list();
}
