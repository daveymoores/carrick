import type { Port } from './port';

export const handlers: Record<string, (port: Port) => Promise<void>> = {
  created: (port) => port.send('created', { id: 1 }),
  deleted: (port) => port.send('deleted'),
};
