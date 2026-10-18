export function isUsersRoute(url: URL): boolean {
  return url.pathname.startsWith("/users");
}
