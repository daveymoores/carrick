// Hand-rolled HTTP client: the base URL is applied inside the wrapper, so
// call sites carry only relative paths + a result generic.
export function makeClient(baseUrl: string) {
  return {
    async get<T>(path: string): Promise<T> {
      const res = await fetch(`${baseUrl}${path}`);
      return res.json() as Promise<T>;
    },
    async patch<T>(path: string, body: unknown): Promise<T> {
      const res = await fetch(`${baseUrl}${path}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      return res.json() as Promise<T>;
    },
  };
}
