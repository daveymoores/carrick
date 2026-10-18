import http from "@fixture/http";

declare function log(o: object): void;
const options = { baseURL: "/o3" };
log(options);
const api = http.create(options);

export function o3(): unknown { return api.get("/users"); }
