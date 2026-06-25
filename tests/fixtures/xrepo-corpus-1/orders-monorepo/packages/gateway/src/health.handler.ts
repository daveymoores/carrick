// Owner-fabrication trap: a raw-handler block that tempts the LLM to emit
// owner = "GET" (the HTTP method string) instead of the real function name.
//
// The handler is defined as a standalone arrow-function constant, then wired up
// in a map-style registration block where the first argument is the method string
// "GET".  A scanner that confuses the method-string argument with the owner will
// emit owner="GET".  The REAL owner is `healthCheckHandler` — the identifier
// bound to the function expression.
//
// Expected: method=GET, path=/gateway/health, owner=healthCheckHandler
// Tier: capability

export interface HealthResponse {
  ok: boolean;
  ts: number;
}

// The real handler function — owner = healthCheckHandler
export const healthCheckHandler = async (
  _req: unknown,
  _res: unknown
): Promise<HealthResponse> => {
  return { ok: true, ts: Date.now() };
};

// Route registry — the method string "GET" appears as the first arg, which is
// the fabrication bait.  The actual route owner is `healthCheckHandler` above.
const routeRegistry: Array<{ method: string; path: string; handler: Function }> = [
  { method: 'GET', path: '/gateway/health', handler: healthCheckHandler },
];

export { routeRegistry };
