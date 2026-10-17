import type { Port } from './port';
import type { Relay } from './relay';
import type { Store } from './store';
import type { Step } from './steps';

export async function run(port: Port, store: Store, relay: Relay, step: Step): Promise<number> {
  await port.send('created', { id: 1 });
  store.save('key', 'value');
  await relay.forward('moved', { id: 2 });
  return step.run();
}
