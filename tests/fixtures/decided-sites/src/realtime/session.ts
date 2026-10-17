import { connect } from "@fixture/wire";

const wire = connect("ws://realtime.internal");

// The caller decides the event; the payload is built here.
export function announce(name: string) {
  wire.emit(name, { at: Date.now() });
}
