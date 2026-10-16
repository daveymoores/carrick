import {
  PREFIX,
  getApiBase,
  sessionBase,
  PATHS,
  pick,
  same,
  MUTABLE,
  viaLet,
  afterLoop,
  itemPath,
} from "./config";

const apiFetch = (url: string) => fetch(url);

export function viaCall() {
  return apiFetch(getApiBase() + "/session");
}
export function viaConst(id: string) {
  return apiFetch(`${PREFIX}/x/${id}`);
}
export function viaLocals() {
  return apiFetch(sessionBase() + "/local");
}
export function viaMember() {
  return apiFetch(PATHS.users + "/m");
}
export function viaTwo(flag: boolean) {
  return apiFetch(pick(flag) + "/two");
}
export function viaSame(flag: boolean) {
  return apiFetch(same(flag) + "/same");
}
export function viaEnv() {
  return apiFetch(process.env.X + "/env");
}
export function retarget() {
  MUTABLE.users = "/other";
}
export function viaMutable() {
  return apiFetch(MUTABLE.users + "/mutable");
}
export function viaLet2() {
  return apiFetch(viaLet() + "/let");
}
export function viaLoop() {
  return apiFetch(afterLoop([]) + "/loop");
}
export function viaText() {
  return apiFetch(itemPath("abc") + "/text");
}
export function viaParam(id: string) {
  return apiFetch(itemPath(id) + "/param");
}
