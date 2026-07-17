// Shared messaging contracts for the wrapper-pattern monorepo fixture.
//
// Three wrapper shapes whose payload types are never a bare named symbol at
// any publish/subscribe call site — the type is carried by a generic binding
// established here:
//   1. TopicBus<Events>: a typed event-emitter over a topic -> payload-tuple map
//   2. QueueWorker<Catalog>: a job worker generic over a catalog of schemas,
//      payload types derived via conditional/mapped types (InferSchema)
//   3. channel<T>(): a handle factory whose payload type is a type argument at
//      the declaration site only
//
// All code is fixture-owned and dependency-free on purpose: the pattern is
// structural, not any particular library.

// ---------------------------------------------------------------------------
// Shape 1: typed event bus over a topic map
// ---------------------------------------------------------------------------

export interface TopicBus<Events extends Record<string, [unknown]>> {
  emit<K extends keyof Events>(topic: K, ...payload: Events[K]): boolean;
  on<K extends keyof Events>(
    topic: K,
    handler: (...payload: Events[K]) => void
  ): this;
}

export function createBus<
  Events extends Record<string, [unknown]>,
>(): TopicBus<Events> {
  const handlers = new Map<keyof Events, Array<(...args: never[]) => void>>();
  return {
    emit(topic, ...payload) {
      const list = handlers.get(topic) ?? [];
      for (const handler of list) {
        (handler as (...args: unknown[]) => void)(...payload);
      }
      return list.length > 0;
    },
    on(topic, handler) {
      const list = handlers.get(topic) ?? [];
      list.push(handler as unknown as (...args: never[]) => void);
      handlers.set(topic, list);
      return this;
    },
  };
}

export type BusEvents = {
  itemArchived: [
    {
      time: Date;
      item: {
        id: string;
        status: string;
        error: { code: string; message: string } | null;
      };
    },
  ];
  itemRestored: [{ time: Date; itemId: string }];
};

export const bus = createBus<BusEvents>();

// ---------------------------------------------------------------------------
// Shape 2: job worker generic over a schema catalog
// ---------------------------------------------------------------------------

type Prim = "string" | "number" | "boolean";

type PrimToTs<P> = P extends "string"
  ? string
  : P extends "number"
    ? number
    : P extends "boolean"
      ? boolean
      : P extends readonly (infer L)[]
        ? L
        : never;

export interface PayloadSchema<T> {
  readonly shape: Record<string, unknown>;
  readonly __out?: T;
}

/** Derive a payload type from a value-level shape, the way schema libraries do. */
export function payloadSchema<
  S extends Record<string, Prim | readonly string[]>,
>(shape: S): PayloadSchema<{ [K in keyof S]: PrimToTs<S[K]> }> {
  return { shape };
}

export type InferSchema<X> = X extends PayloadSchema<infer T> ? T : never;

export type JobCatalog = {
  [key: string]: { schema: PayloadSchema<unknown> };
};

export type JobHandler<C extends JobCatalog, K extends keyof C> = (params: {
  id: string;
  payload: InferSchema<C[K]["schema"]>;
}) => Promise<void>;

export class QueueWorker<C extends JobCatalog> {
  constructor(
    private readonly opts: {
      catalog: C;
      jobs: { [K in keyof C]: JobHandler<C, K> };
    }
  ) {}

  async enqueue<K extends keyof C>(args: {
    id: string;
    job: K;
    payload: InferSchema<C[K]["schema"]>;
  }): Promise<void> {
    await this.opts.jobs[args.job]({ id: args.id, payload: args.payload });
  }
}

export const jobCatalog = {
  "records.reindex": {
    schema: payloadSchema({
      resourceId: "string",
      mode: ["full", "partial"],
    } as const),
  },
} satisfies JobCatalog;

// ---------------------------------------------------------------------------
// Shape 3: generic channel-handle factory
// ---------------------------------------------------------------------------

export interface ChannelHandle<T> {
  send(target: string, data: T): Promise<void>;
  on(cb: (data: T) => void): void;
}

export function channel<T>(opts: { id: string }): ChannelHandle<T> {
  const subscribers: Array<(data: T) => void> = [];
  void opts;
  return {
    async send(_target: string, data: T) {
      for (const cb of subscribers) {
        cb(data);
      }
    },
    on(cb: (data: T) => void) {
      subscribers.push(cb);
    },
  };
}
