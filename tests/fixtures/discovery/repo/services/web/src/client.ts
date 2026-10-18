import { request } from "./request";

export const client = {
  load: (o: RequestInit) => request("/items", o),
};
