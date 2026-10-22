import { App, Reply, Request } from "fixture-router";

function ok(_request: Request, reply: Reply): void {
  reply.send("ok");
}

interface ExtraOptions {
  extra: (app: App) => void;
}

function withExtra(app: App, options: ExtraOptions): void {
  options.extra(app);
}

export function withOptions(app: App): void {
  withExtra(app, { extra: (inner) => inner.get("/extra", ok) });
}
