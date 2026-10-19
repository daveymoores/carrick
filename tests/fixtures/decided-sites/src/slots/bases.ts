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
