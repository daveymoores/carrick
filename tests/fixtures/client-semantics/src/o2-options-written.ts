import http from "@fixture/http";

declare const other: string;
const options = { baseURL: "/o2" };
options.baseURL = other;
const api = http.create(options);

export function o2(): unknown { return api.get("/users"); }
