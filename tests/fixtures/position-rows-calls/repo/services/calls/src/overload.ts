export function readPath(path: "/a"): Promise<Response>;
export function readPath(path: string): Promise<Response>;
export function readPath(path: string): Promise<Response> {
  return fetch(path);
}
