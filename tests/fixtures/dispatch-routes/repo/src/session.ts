export function requireUser(request: Request): string | null {
  if (new URL(request.url).pathname === "/logout") return null;
  return request.headers.get("x-user");
}
