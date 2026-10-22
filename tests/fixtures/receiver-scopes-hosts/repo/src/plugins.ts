import type { Admin, App } from "fixture-twin-server";
import type { Hub } from "fixture-split-server";
import { items } from "./items";
import { list } from "./list";
import { things } from "./things";

// Plugins nothing here calls: each one's server is whatever runs it.

// Two exports of one package each have a `register`: this one is the app's.
export async function appPlugin(app: App) {
  app.register(items, { prefix: "/app" });
}

// And this one the admin's, whose instances are constructed.
export async function adminPlugin(admin: Admin) {
  admin.register(list, { prefix: "/admin" });
}

// A server whose types a package of their own declares.
export async function hubPlugin(hub: Hub) {
  hub.register(things, { prefix: "/hub" });
}
