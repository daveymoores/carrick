import { request } from "@fixture/http";

export function check() {
  request("/status", { method: "GET" }, (res) => res.status);
}
