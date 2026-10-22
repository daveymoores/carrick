import type { Port } from './port';

export class HttpPort implements Port {
  send(event: string, body?: unknown): Promise<void> {
    return Promise.resolve(void [event, body]);
  }
}
