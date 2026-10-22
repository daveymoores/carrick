import type { Port } from './port';

export const memoryPort = {
  async send(event: string, body?: unknown): Promise<void> {
    void [event, body];
  },
} satisfies Port;
