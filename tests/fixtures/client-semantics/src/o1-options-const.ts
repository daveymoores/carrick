import http from "@fixture/http";

const options = { baseURL: "/o1" };
const api = http.create(options);

export function o1(): unknown { return api.get("/users"); }
