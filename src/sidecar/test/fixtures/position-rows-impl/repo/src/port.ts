export interface Port {
  send(name: string, body: unknown): Promise<void>;
}

export class HttpPort implements Port {
  async send(name: string, body: unknown): Promise<void> {
    await fetch(`/events/${name}`, { method: 'POST', body: JSON.stringify(body) });
  }
}

declare const worker: { postMessage(message: unknown): void };

export const workerPort: Port = {
  send(name, body) {
    worker.postMessage({ name, body });
    return Promise.resolve();
  },
};
