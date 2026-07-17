// function_param locator shapes for pub/sub wrapper subscribers: the payload
// type is carried by a generic binding (topic map / schema catalog / handle
// factory), never a named annotation at the handler. Self-contained on
// purpose — the wrapper is structural, not any library.

type BusEvents = {
  itemArchived: [
    {
      time: Date;
      item: { id: string; status: string };
    },
  ];
};

interface TopicBus<Events extends Record<string, [unknown]>> {
  on<K extends keyof Events>(
    topic: K,
    handler: (...payload: Events[K]) => void
  ): void;
}

declare const bus: TopicBus<BusEvents>;

export function watchArchives(): void {
  // Whole destructured binding pattern: the param IS the payload.
  bus.on("itemArchived", ({ time, item }) => {
    console.log(time.toISOString(), item.id);
  });
}

interface JobEnvelope<T> {
  id: string;
  payload: T;
}

interface JobRunner<T> {
  register(handler: (params: JobEnvelope<T>) => Promise<void>): void;
}

declare const reindexRunner: JobRunner<{ resourceId: string; mode: "full" | "partial" }>;

export function registerReindex(): void {
  // Envelope param: the payload is ONE binding element of the destructured
  // param — the locator names the element, and the checker projects its type.
  reindexRunner.register(async ({ payload }) => {
    console.log(payload.resourceId, payload.mode);
  });
}
