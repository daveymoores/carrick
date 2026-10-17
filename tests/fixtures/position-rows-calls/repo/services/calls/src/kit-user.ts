import { kit } from "http-kit";
import { makeClient } from "fixture-kit";

export function notify(): void {
  kit.send("kit.topic");
}

export function viaLocal(): Promise<unknown> {
  const client = [makeClient()][0];
  return client.get("/kit");
}
