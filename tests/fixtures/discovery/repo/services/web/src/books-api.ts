import { request } from "./request";

export function listBooks(): Promise<Response> {
  return request("/books");
}
