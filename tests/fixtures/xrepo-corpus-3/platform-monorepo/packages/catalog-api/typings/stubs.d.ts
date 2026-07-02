// Ambient stubs — keep types resolvable without npm install.
// Just enough surface for the call sites; the scanner reads AST structure.

declare module "koa" {
  class Koa {
    use(mw: unknown): this;
    listen(port: number): void;
  }
  export = Koa;
}

declare module "@koa/router" {
  export interface RouterContext {
    params: Record<string, string>;
    body: unknown;
    status: number;
    request: { body: unknown };
  }
  type Middleware = (ctx: RouterContext) => Promise<void> | void;
  class Router {
    get(path: string, mw: Middleware): this;
    patch(path: string, mw: Middleware): this;
    del(path: string, mw: Middleware): this;
    routes(): unknown;
    allowedMethods(): unknown;
  }
  export = Router;
}

declare module "nats" {
  export interface NatsConnection {
    publish(subject: string, data: Uint8Array): void;
    close(): Promise<void>;
  }
  export function connect(opts?: { servers?: string | string[] }): Promise<NatsConnection>;
  export interface Codec<T> {
    encode(d: T): Uint8Array;
    decode(a: Uint8Array): T;
  }
  export function StringCodec(): Codec<string>;
}

declare module "supertest" {
  interface Test {
    expect(status: number): Promise<unknown>;
  }
  interface Agent {
    get(path: string): Test;
    post(path: string): Test;
  }
  function request(app: unknown): Agent;
  export = request;
}
