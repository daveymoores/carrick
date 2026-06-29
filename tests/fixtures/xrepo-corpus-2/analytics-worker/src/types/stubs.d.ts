// Ambient stubs — keep types resolvable without npm install.

// ioredis stub — dual-use client: pub/sub (publish) AND key-value (set/get).
// The KV surface exists only so the DECOY in redis/publisher.ts type-checks.
declare module "ioredis" {
  export default class Redis {
    constructor(url?: string);
    publish(channel: string, message: string): Promise<number>;
    set(key: string, value: string): Promise<"OK">;
    get(key: string): Promise<string | null>;
  }
}

// nats.js stub — connection + JSONCodec for the publish path.
declare module "nats" {
  export interface Codec<T> {
    encode(value: T): Uint8Array;
    decode(data: Uint8Array): T;
  }
  export function JSONCodec<T = unknown>(): Codec<T>;
  export interface NatsConnection {
    publish(subject: string, payload: Uint8Array): void;
  }
  export function connect(opts?: { servers?: string }): Promise<NatsConnection>;
}

// Hono stub — minimal context with a generic json() response helper.
declare module "hono" {
  export interface Context {
    req: {
      json<T = unknown>(): Promise<T>;
    };
    json<T = unknown>(body: T, status?: number): T;
  }
  export class Hono {
    post(path: string, handler: (c: Context) => unknown): Hono;
    get(path: string, handler: (c: Context) => unknown): Hono;
  }
}
