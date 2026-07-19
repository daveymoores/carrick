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

// ---------------------------------------------------------------------------
// Multi-occurrence disambiguation: the SAME locator text (`payloadValue`)
// appears at two publish sites with different types. A line-anchored
// expression search must resolve each site's own type; an unanchored search
// has no proximity signal and can bind the wrong occurrence.
// ---------------------------------------------------------------------------

declare function send(topic: string, payload: unknown): boolean;

export function publishFirst(): void {
  const payloadValue = { kind: "first", n: 1 };
  void send("first.topic", payloadValue);
}

// Spacer comments keep the two occurrences farther apart than the text
// search's +/-5-line window, so each anchored request can only see its own
// site's occurrences.
//
//
//
//

export function publishSecond(): void {
  const payloadValue = { kind: "second", s: "x" };
  void send("second.topic", payloadValue);
}

// ---------------------------------------------------------------------------
// Named-annotation payloads for the two-anchor arbitration (carrick#413):
// the subscriber param and the publisher argument both carry a NAMED payload
// type, so the deterministic anchor (primary_type_symbol + its declaration
// source file) must be reported alongside the named type string.
// ---------------------------------------------------------------------------

export interface OrderPlacedPayload {
  orderId: string;
  totalCents: number;
}

export function onOrderPlaced(evt: OrderPlacedPayload): void {
  void evt;
}

export function publishOrder(): void {
  const order: OrderPlacedPayload = { orderId: "o1", totalCents: 100 };
  void send("orders.placed", order);
}
