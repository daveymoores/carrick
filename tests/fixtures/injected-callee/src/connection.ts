// 8. A parameter's field or the platform's `fetch`, chosen inline at the
// call.
export interface Connection {
  fetchImpl?: typeof fetch;
}

export function fetchStatus(connection: Connection): Promise<Response> {
  return (connection.fetchImpl ?? fetch)("/api/status", { method: "GET" });
}

export function checkStatus(): Promise<Response> {
  return fetchStatus({});
}
