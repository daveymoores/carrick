// carrick#2524: a base the code reads by a name is written by that name, or
// not at all; never as the source text of an expression.

// A module constant whose initializer is an expression over an environment
// read.
const SEED_BASE = (process.env.SEED_API || "http://localhost:3000/api").replace(/\/$/, "");

export function signIn(): Promise<Response> {
  return fetch(`${SEED_BASE}/sessions`, { method: "POST" });
}

// A module constant that is the environment read itself.
const PLAIN_BASE = process.env.PLAIN_API;

export function signOut(): Promise<Response> {
  return fetch(`${PLAIN_BASE}/sessions/end`, { method: "POST" });
}

// A constant over two environment reads: which one is the base is not
// stated.
const EITHER_BASE = (process.env.PRIMARY_API || process.env.BACKUP_API || "").trim();

export function status(): Promise<Response> {
  return fetch(`${EITHER_BASE}/status`);
}

// A constant over a value with no environment read.
declare const settings: { base: string };
const TRIMMED = settings.base.replace(/\/$/, "");

export function ping(): Promise<Response> {
  return fetch(`${TRIMMED}/ping`);
}
