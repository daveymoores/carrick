import make, { App, Request, Response, shared } from "fixture-server";
import { build } from "./build";

function ok(_request: Request, response: Response): void {
  response.send("ok");
}

// One helper handed two apps, and a factory's app built in another module.
export function health(app: App): void {
  app.get("/health", ok);
}

// A chain three helpers deep.
function three(app: App): void {
  app.get("/deep", ok);
}
function two(app: App): void {
  three(app);
}
function one(app: App): void {
  two(app);
}

// A cycle: each calls the other.
function ping(app: App, n: number): void {
  if (n > 0) {
    pong(app, n - 1);
  }
  app.get("/ping", ok);
}
function pong(app: App, n: number): void {
  ping(app, n);
}

// A function nothing calls.
export function orphan(app: App): void {
  app.get("/orphan", ok);
}

// A key of a parameter.
function keyed(deps: { app: App }): void {
  deps.app.get("/keyed", ok);
}

// A function the library runs, handed to it by name.
function plugin(scoped: App): void {
  scoped.get("/plugin", ok);
}

export function start(): void {
  const main = make();
  const admin = make();
  health(main);
  health(admin);
  health(build());
  health(shared);
  one(main);
  ping(main, 2);
  keyed({ app: main });
  main.register(plugin);
  main.register((inner) => inner.get("/inline", ok));
  health(build());
}
