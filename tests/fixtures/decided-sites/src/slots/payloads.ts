// carrick#2674: a request's body is read where the code that calls the
// wrapper writes it. A wrapper hands its caller's options on through a spread
// or the rest of a destructured parameter; the caller's object states the
// body, or states that none is written.

function traced<T>(run: () => T): T {
  return run();
}

// The rest of a destructured options parameter, spread into the request.
function send(config: { path: string; method?: string; body?: string; headers?: object }) {
  const { path, headers, ...rest } = config;
  return fetch(`${process.env.API}${path}`, { ...rest, credentials: "include" });
}

export function addNote(note: object): Promise<Response> {
  return send({ path: "/notes", method: "POST", body: JSON.stringify(note) });
}

export function ping(): Promise<Response> {
  return send({ path: "/ping", method: "POST" });
}

// The same rest, with the request inside a closure.
function sendTraced(config: { path: string; method?: string; body?: string }) {
  const { path, ...tracedRest } = config;
  return traced(() =>
    fetch(`${process.env.API}${path}`, { ...tracedRest, credentials: "include" }),
  );
}

export function addTracedNote(note: object): Promise<Response> {
  return sendTraced({ path: "/traced", method: "POST", body: JSON.stringify(note) });
}

export function pingTraced(): Promise<Response> {
  return sendTraced({ path: "/traced/ping", method: "POST" });
}

// The rest of an optional parameter, in a closure.
function call(path: string, init?: { method?: string; body?: string; retries?: number }) {
  const { retries, ...requestInit } = init ?? {};
  return traced(() =>
    fetch(`${process.env.API}${path}`, { ...requestInit, credentials: "include" }),
  );
}

export function removeNote(id: string): Promise<Response> {
  return call(`/notes/${id}`, { method: "DELETE" });
}

export function retitleNote(id: string, title: string): Promise<Response> {
  return call(`/notes/${id}/title`, { method: "PUT", body: JSON.stringify({ title }) });
}

// A spread of the function's own parameter.
function post(path: string, init: { body?: string }) {
  return fetch(`${process.env.API}${path}`, { method: "POST", ...init });
}

export function logout(): Promise<Response> {
  return post("/logout", {});
}

export function addTag(tag: string): Promise<Response> {
  return post("/tags", { body: tag });
}

// A direct call that writes a closed literal.
export function directPing(): Promise<Response> {
  return fetch(`${process.env.API}/direct/ping`, { method: "POST" });
}

export function directBody(body: string): Promise<Response> {
  return fetch(`${process.env.API}/direct/body`, { method: "POST", body });
}

// Controls: the key is not read.
function getDefaults(): object {
  return { headers: {} };
}

export function spreadsAnother(): Promise<Response> {
  return call("/x", { ...getDefaults(), method: "POST" });
}

function sendStripped(config: { path: string; body?: string }) {
  const { path, body, ...bodiless } = config;
  return fetch(`${process.env.API}${path}`, { ...bodiless, credentials: "include" });
}

export function strippedCaller(): Promise<Response> {
  return sendStripped({ path: "/stripped" });
}

function withDefaults(init: object): object {
  return { ...init, credentials: "include" };
}

function wrapped(path: string, init: object) {
  return fetch(`${process.env.API}${path}`, withDefaults(init));
}

export function wrappedCaller(): Promise<Response> {
  return wrapped("/wrapped", { method: "POST" });
}

export function directSpread(): Promise<Response> {
  return fetch(`${process.env.API}/direct/spread`, { ...getDefaults(), method: "POST" });
}
