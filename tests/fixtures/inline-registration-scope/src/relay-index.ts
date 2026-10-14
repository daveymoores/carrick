import { plugin } from './relay-local';

export async function main(server: any) {
  server.register(plugin, { prefix: '/root' });
}
