const base = () => "/api/v1";
function sessionBase() {
  const root = "/api";
  return root + "/v1";
}
function pick(flag: boolean) {
  if (flag) {
    return "/a";
  }
  return "/b";
}
function same(flag: boolean) {
  if (flag) {
    return "/a";
  }
  return "/a";
}
const apiFetch = (url: string) => fetch(url);

export function localCall() {
  return apiFetch(base() + "/session");
}
export function localLocals() {
  return apiFetch(sessionBase() + "/local");
}
export function localTwo(flag: boolean) {
  return apiFetch(pick(flag) + "/two");
}
export function localSame(flag: boolean) {
  return apiFetch(same(flag) + "/same");
}
