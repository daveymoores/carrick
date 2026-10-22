import { HttpPort } from './http-port';
import { QueuePort } from './queue-port';
import { memoryPort } from './memory-port';

export async function direct(): Promise<void> {
  await new HttpPort().send('a');
  await new QueuePort().send('b');
  await memoryPort.send('c');
}
