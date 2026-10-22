import { isReport } from "./report";

function resolve(request: Request): string {
  return new URL(request.url).pathname;
}

export function check(request: Request): Response {
  if (isReport(resolve(request))) return new Response("report");
  return new Response("no", { status: 404 });
}
