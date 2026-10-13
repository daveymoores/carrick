// The control: a base the file really reads from the environment.
const HEALTH_BASE = process.env.REAL_URL;

export async function checkHealth(): Promise<string> {
  const res = await fetch(`${HEALTH_BASE}/health`, { method: "GET" });
  return res.text();
}
