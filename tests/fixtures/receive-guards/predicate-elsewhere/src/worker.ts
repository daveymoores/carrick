import { isUsersRoute } from "./routes";
import { isMediaRoute } from "./media.js";
import { isAdminRoute } from "@/lib/admin";

async function answer(request: Request, url: URL): Promise<Response> {
  if (isUsersRoute(url)) {
    return handleUsers(request);
  }
  return new Response("not found", { status: 404 });
}

function handleUsers(request: Request): Response {
  return Response.json({ method: request.method });
}

export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (!isAdminRoute(url)) {
      return new Response("forbidden", { status: 403 });
    }
    if (isMediaRoute(url)) {
      return new Response("media");
    }
    return answer(request, url);
  },
};
