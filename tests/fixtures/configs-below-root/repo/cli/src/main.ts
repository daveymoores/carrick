import { fetchStatus } from './util.js';

export async function ping(): Promise<number> {
  return fetchStatus('/api/ping');
}
