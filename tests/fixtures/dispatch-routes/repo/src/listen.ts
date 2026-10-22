import { createServer } from "node:http";
import type { RequestListener } from "node:http";

export function listen(handler: RequestListener): void {
  createServer(handler).listen(8080);
}
