import { connect, Wire } from "@fixture/wire";

export class Session {
  constructor(private readonly wire: Wire) {}

  notify(name: string, data: unknown): void {
    console.log(name);
    relay(this.wire, name, data);
  }
}

function relay(wire: Wire, name: string, data: unknown): void {
  wire.emit(name, data);
}

export function openSession(url: string): Session {
  return new Session(connect(url));
}
