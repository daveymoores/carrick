// Telemetry beacon — uses navigator.sendBeacon (browser built-in).
// No import: sendBeacon is a global URL-transmitting function, not a library.

export interface MetricPayload {
  event: string;
  paymentId: string;
  durationMs: number;
}

export function reportMetric(payload: MetricPayload): boolean {
  const body = JSON.stringify(payload);
  return navigator.sendBeacon("/metrics/ingest", body);
}
