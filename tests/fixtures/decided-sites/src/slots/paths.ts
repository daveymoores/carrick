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

// A whole segment a caller states stays at the caller.
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
