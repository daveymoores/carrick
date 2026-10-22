import type { Handler, Request } from "fixture-server";

// (f) the request's own `get`: the same package, another declaration.
export function readId(request: Request): string | undefined {
  return request.get("x-id");
}

// (g) an in-repo class with its own `get(path, handler)`.
class Local {
  get(path: string, handler: () => void): void {
    void path;
    handler();
  }
}
new Local().get("/g", () => {});

// (h) an untyped receiver.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function loose(target: any): void {
  target.get("/h", () => {});
}

// (i) a local structural type copying the app's `get`.
interface Shape {
  get(path: string, handler: Handler): Shape;
}
export function shaped(shape: Shape, handler: Handler): void {
  shape.get("/i", handler);
}
