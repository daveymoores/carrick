export const PREFIX = "/api/v1";
export const getApiBase = () => "/api/v1";
export function sessionBase() {
  const root = "/api";
  return root + "/v1";
}
export const PATHS = { users: "/users" } as const;
export function pick(flag: boolean) {
  if (flag) {
    return "/a";
  }
  return "/b";
}
export function same(flag: boolean) {
  if (flag) {
    return "/a";
  }
  return "/a";
}
export const MUTABLE = { users: "/users" };
export function viaLet() {
  let value = "/a";
  return value;
}
export function afterLoop(items: string[]) {
  for (const item of items) {
    console.log(item);
  }
  return "/a";
}
export function itemPath(segment: string) {
  return "/items/" + segment;
}
