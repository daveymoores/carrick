import type { FakeThing } from 'fakelib';
import { makeThing } from 'fakelib';

// Annotation through the (missing-on-bare) external: the amendment-1
// allowlist case when anchored directly.
export type ShipmentThing = FakeThing;

// Inference THROUGH the missing library bakes any on a bare checkout (the
// honest limitation from the design doc).
export const DerivedConfig = makeThing('cfg');
export type DerivedConfigType = typeof DerivedConfig;

export function run(publish: (topic: string, payload: unknown) => void): void {
  publish('order.shipped', { orderId: '1', eta: 'soon' });
}

async function fetchShipment(): Promise<{ shipmentId: string; ok: boolean }> {
  return { shipmentId: 's1', ok: true };
}

export function runAsync(publish: (topic: string, payload: unknown) => void): void {
  publish('shipment.sync', fetchShipment());
}
