import { ping } from "./health";

export function healthCheck(): Promise<Response> {
  return ping("/api/health", { send: fetch });
}
