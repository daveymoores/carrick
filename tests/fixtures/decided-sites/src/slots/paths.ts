// carrick#2519: a request's path composes from the parts the code states.

const API = process.env.API;

// A wrapper adds an environment base; its caller writes the path as a
// template holding its own parameter, and nothing calls the caller.
function send(path: string, method: string): Promise<Response> {
  return fetch(`${process.env.API}${path}`, { method });
}

export function setStatus(taskId: string): Promise<Response> {
  return send(`/tasks/${taskId}/status`, "PUT");
}

// A base set in the constructor, a local URL with two path parameters.
export class Files {
  private baseUrl: string;

  constructor(config: { url: string }) {
    this.baseUrl = config.url;
  }

  download(resourceId: string, fileId: string): Promise<Response> {
    const url = `${this.baseUrl}/resources/${resourceId}/files/${fileId}`;
    return fetch(url);
  }
}

// A wrapper whose whole path is a parameter: each caller states it.
function get(path: string): Promise<Response> {
  return fetch(`${API}${path}`);
}

export function listOrders(): Promise<Response> {
  return get("/orders");
}

export function listRefunds(): Promise<Response> {
  return get("/refunds");
}

// A path parameter every caller passes literal text: each caller is a site,
// with its literal.
function readOne(id: string): Promise<Response> {
  return fetch(`/accounts/${id}`);
}

export function readMine(): Promise<Response> {
  return readOne("me");
}

// A parameter that is the whole path, with no caller, stays a lead.
export function relay(path: string): Promise<Response> {
  return fetch(path);
}

// A wrapper spreads its caller's options into the request: the method is
// the one the caller's object writes.
function call(path: string, init: { method?: string }): Promise<Response> {
  return fetch(`${process.env.API}${path}`, { ...init, credentials: "include" });
}

export function archive(id: string): Promise<Response> {
  return call(`/archives/${id}`, { method: "POST" });
}

// A value in the query is no path parameter: the route is the path before
// the `?`.
export function findGroup(id: string, workspace: string): Promise<Response> {
  return fetch(`/groups/${id}?workspace=${workspace}`);
}

// A path parameter some caller fills from data stays a path parameter at
// the call.
function readUser(id: string): Promise<Response> {
  return fetch(`/users/${id}`);
}

export function readFirst(): Promise<Response> {
  return readUser("first");
}

export function readFrom(user: { id: string }): Promise<Response> {
  return readUser(user.id);
}

// The rest of a destructured options parameter, spread into the request:
// the method is the one the caller's object writes.
function sendNote(config: { path: string; method?: string; headers?: object }) {
  const { path, headers, ...rest } = config;
  return fetch(`${process.env.API}${path}`, { ...rest, credentials: "include" });
}

export function removeNote(id: string): Promise<Response> {
  return sendNote({ path: `/notes/${id}`, method: "DELETE" });
}

function traced<T>(run: () => T): T {
  return run();
}

function closeWith(path: string, init?: { method?: string; retries?: number }) {
  const { retries, ...requestInit } = init ?? {};
  return traced(() =>
    fetch(`${process.env.API}${path}`, { ...requestInit, credentials: "include" }),
  );
}

export function closeNote(id: string): Promise<Response> {
  return closeWith(`/notes/${id}/close`, { method: "PATCH" });
}

// A parameter every caller passes literal text, in the query only: the
// route is decided at the call, not at its callers.
function pageOf(page: number): Promise<Response> {
  return fetch(`/dashboard?page=${page}`);
}

export function firstPage(): Promise<Response> {
  return pageOf("1" as unknown as number);
}
