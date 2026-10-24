import { span } from "@fixture/trace";
import { env } from "./env";

// The request is made inside a function handed to a package's export.
export const send = (path: string, init?: RequestInit): Promise<Response> =>
  span(`request ${path}`, () => fetch(`${env.API_URL}${path}`, init));
