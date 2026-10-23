// 4. The function an in-repo factory returns, called where it is held.
export function makeHttp(): (method: string, path: string) => Promise<Response> {
  return (method, path) => fetch(path, { method });
}

export function listItems(): Promise<Response> {
  const http = makeHttp();
  return http("GET", "/api/items");
}
