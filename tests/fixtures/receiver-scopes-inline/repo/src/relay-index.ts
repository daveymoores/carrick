import type { App } from 'fixture-plugin-server';
import { plugin } from './relay-local';

export async function main(server: App) {
  server.register(plugin, { prefix: '/root' });
}
