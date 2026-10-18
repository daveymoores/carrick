export function isAdminRoute(url: URL): boolean {
  return url.pathname.startsWith("/admin");
}
