import { Reply, Request } from "fixture-router";
import { shared } from "./shared";

function ok(_request: Request, reply: Reply): void {
  reply.send("ok");
}

// The instance another module exports, used here too.
shared.post("/pong", ok);
