// Ambient stubs — keep types resolvable without npm install.
// Just enough surface for the call sites; the scanner reads AST structure.

declare module "bullmq" {
  export interface Job<T = unknown> {
    data: T;
  }
  export class Queue<T = unknown> {
    constructor(name: string, opts?: { connection?: unknown });
    add(jobName: string, data: T): Promise<unknown>;
  }
  export class Worker<T = unknown> {
    constructor(
      name: string,
      processor: (job: Job<T>) => Promise<void>,
      opts?: { connection?: unknown }
    );
  }
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
  export function JSONCodec<T>(): Codec<T>;
}

declare module "got" {
  interface ResponsePromise {
    json<T>(): Promise<T>;
  }
  interface Got {
    get(url: string): ResponsePromise;
    post(url: string, opts?: { json?: unknown }): ResponsePromise;
  }
  const got: Got;
  export default got;
}
