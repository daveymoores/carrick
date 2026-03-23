import express from "express";
import serverlessExpress from "@codegenie/serverless-express";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { ApiClient } from "./api-client.js";
import { createServer } from "./server.js";

const getRequiredEnv = (name: string): string => {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Missing required environment variable: ${name}`);
  }
  return value;
};

// Module-level client — persists across warm Lambda invocations so
// the 5-min in-memory cache survives between requests.
const client = new ApiClient({
  apiEndpoint: getRequiredEnv("CARRICK_API_ENDPOINT"),
  apiKey: getRequiredEnv("CARRICK_API_KEY"),
  org: getRequiredEnv("CARRICK_ORG"),
});

const VALID_KEYS = new Set(
  (process.env.VALID_API_KEYS || "").split(",").map((k) => k.trim()).filter(Boolean),
);

const app = express();
app.use(express.json());

// Auth middleware — same Bearer-token pattern as existing Lambdas
app.use("/mcp", (req, res, next) => {
  const auth = req.headers.authorization;
  if (!auth?.startsWith("Bearer ")) {
    res.status(401).json({ error: "Missing Bearer token" });
    return;
  }
  const token = auth.slice(7);
  if (!VALID_KEYS.has(token)) {
    res.status(403).json({ error: "Invalid API key" });
    return;
  }
  next();
});

// POST /mcp — Streamable HTTP transport (stateless, JSON responses)
app.post("/mcp", async (req, res) => {
  try {
    const transport = new StreamableHTTPServerTransport({
      sessionIdGenerator: undefined, // stateless
      enableJsonResponse: true, // JSON, not SSE — avoids Lambda streaming issues
    });

    const server = createServer(client);
    await server.connect(transport);
    await transport.handleRequest(req, res, req.body);

    // Clean up after the request is handled
    await server.close();
  } catch (err) {
    console.error("MCP request error:", err);
    if (!res.headersSent) {
      res.status(500).json({ error: "Internal server error" });
    }
  }
});

// GET /mcp and DELETE /mcp — forward-compatible stubs for session management
app.get("/mcp", (_req, res) => {
  res.status(405).json({
    jsonrpc: "2.0",
    error: { code: -32000, message: "Stateless server, sessions not supported" },
    id: null,
  });
});

app.delete("/mcp", (_req, res) => {
  res.status(405).json({
    jsonrpc: "2.0",
    error: { code: -32000, message: "Stateless server, sessions not supported" },
    id: null,
  });
});

export const handler = serverlessExpress({ app });
