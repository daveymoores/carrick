import { remote } from "@calls/remote";
import { Child, Users } from "./models";
import { readPath } from "./overload";
import { registry } from "./registry";
import { ping } from "./index";
import { Reports } from "./reports";

function users(): Users {
  return new Users();
}

export function loadUsers(): Promise<Response> {
  return registry.users.list();
}

export function loadChild(child: Child): Promise<Response> {
  return child.load();
}

export function loadA(): Promise<Response> {
  return readPath("/a");
}

export function loadAny(client: any): Promise<Response> {
  return client.list();
}

export function callPing(): Promise<Response> {
  return ping();
}

export function loadReports(reports: Reports): Promise<Response> {
  return reports.fetchAll();
}

export function callRemote(): Promise<Response> {
  return remote();
}

export function chained(): Promise<Response> {
  return users().list();
}
