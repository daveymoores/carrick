import { Socket } from "fixture-socket";

export class Session {
  constructor(private readonly socket: Socket) {}

  notify(name: string, data: unknown): void {
    this.socket.send(name, data);
  }
}

export function openSession(url: string): Session {
  return new Session(new Socket(url));
}
