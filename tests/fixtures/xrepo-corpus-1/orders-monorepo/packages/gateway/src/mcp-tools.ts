// MCP tool registration decoy — MUST NOT emit any HTTP endpoints or calls.
//
// This file registers tools on an MCP server using the server.tool(name, schema,
// handler) pattern.  The registration call looks superficially like a route
// registration (string name, object schema, async handler) but it is NOT an HTTP
// endpoint.  The scanner must not extract these as endpoints.
//
// The _must_not_emit guard in expected.json uses a synthetic path marker
// /__mcp__/lookup-order that the scanner would never emit from real HTTP code;
// the real guard is set-absence: the total expected endpoint set is exact, so any
// MCP-derived emission reduces precision even without the synthetic marker.

interface McpTool {
  name: string;
  schema: object;
  handler: (args: unknown) => Promise<unknown>;
}

// Minimal stub for an MCP server — no real import needed; the scanner reads AST.
const server = {
  tool(name: string, schema: object, handler: (args: unknown) => Promise<unknown>): void {
    // no-op in this stub
  },
};

// MCP tool: lookup-order — NOT an HTTP endpoint
server.tool(
  'lookup-order',
  {
    type: 'object',
    properties: {
      orderId: { type: 'string' },
    },
    required: ['orderId'],
  },
  async (args: unknown) => {
    const { orderId } = args as { orderId: string };
    return { id: orderId, status: 'shipped' };
  }
);

// MCP tool: cancel-order — NOT an HTTP endpoint
server.tool(
  'cancel-order',
  {
    type: 'object',
    properties: {
      orderId: { type: 'string' },
      reason: { type: 'string' },
    },
    required: ['orderId'],
  },
  async (args: unknown) => {
    const { orderId } = args as { orderId: string; reason?: string };
    return { orderId, cancelled: true };
  }
);

export { server };
