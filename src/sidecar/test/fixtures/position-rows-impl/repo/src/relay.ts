import { workerPort } from './port';

export class Relay {
  constructor(private readonly deliver: (name: string, body: unknown) => Promise<void>) {}

  forward(name: string, body: unknown): Promise<void> {
    return this.deliver(name, body);
  }
}

export const relay = new Relay((name, body) => workerPort.send(name, body));
