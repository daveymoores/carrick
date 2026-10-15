type Handler = (payload: unknown) => void;

function store(payload: unknown): void {
  console.log("store", payload);
}

function drop(payload: unknown): void {
  console.log("drop", payload);
}

const HANDLERS: Record<string, Handler> = {
  save: (payload) => store(payload),
  remove: (payload) => drop(payload),
};

export function dispatch(event: { type: string; payload: unknown }): void {
  const handler = HANDLERS[event.type];
  if (handler) {
    handler(event.payload);
  }
}
