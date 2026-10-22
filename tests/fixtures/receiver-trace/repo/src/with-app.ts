import make, { App } from "fixture-server";

export function withApp(fn: (app: App) => void): void {
  fn(make());
}
