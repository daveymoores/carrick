import { route, use } from "fixture-router";
import type { RouterRequest, RouterResponse } from "fixture-router";

export const stats = (req: RouterRequest, res: RouterResponse): void => {
  if (req.path === "/stats") res.send(1);
};

route("/admin", stats);

use({ strict: true }, (req: RouterRequest, res: RouterResponse) => {
  if (req.path === "/slot") res.send(2);
});

function trimmed(path: string): string {
  return path.replace(/\/$/, "");
}

route("/files", (req: RouterRequest, res: RouterResponse) => {
  const at = trimmed(req.path);
  if (at === "/files/list") res.send(3);
});
