import express from "express";
import type { Run } from "./types";
const app = express();
app.use(express.json());

app.get("/api/v1/runs", (_req, res) => {
  res.json([{ id: "run-1", state: "done" }]);
});

app.get("/api/v1/runs/:id", (req, res) => {
  res.json({ id: req.params.id, state: "done" } satisfies Run);
});

app.put("/api/v1/runs/:id", (req, res) => {
  res.json({ id: req.params.id, state: req.body.state } satisfies Run);
});

app.get("/api/v1/status", (_req, res) => {
  res.json({ healthy: true });
});

app.get("/api/v1/token", (_req, res) => {
  res.json({ token: "t" });
});

app.post("/api/v1/audit", (_req, res) => {
  res.status(204).end();
});

app.get("/health", (_req, res) => {
  res.json({ ok: true });
});

app.put("/api/v1/runs/:id/archive", (req, res) => {
  res.json({ id: req.params.id, state: "archived" });
});

app.listen(4100);
