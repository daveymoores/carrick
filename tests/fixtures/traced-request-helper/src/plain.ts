import { env } from "./env";

// The same helper with no span around the request.
export const sendPlain = (path: string, init?: RequestInit): Promise<Response> =>
  fetch(`${env.API_URL}${path}`, init);
