import { createClient } from './client';

const client = createClient();

export async function load(): Promise<unknown> {
  return client.get('/items');
}

export async function save(): Promise<unknown> {
  return client.post('/items');
}
