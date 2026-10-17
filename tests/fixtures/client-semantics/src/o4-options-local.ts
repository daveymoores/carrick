import http from "@fixture/http";

export function o4(): unknown {
  const options = { baseURL: "/o4" };
  const api = http.create(options);
  return api.get("/users");
}
