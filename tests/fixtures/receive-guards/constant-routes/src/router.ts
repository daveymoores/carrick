import { Router } from "invented-router";
import type { Handler } from "invented-router";
import { LOOKUP_ROUTE } from "./routes";

function authenticate(req: { headers: Record<string, string> }, next: () => void): void {
  if (req.headers.authorization) {
    next();
  }
}

function lookup(req: { params: { id: string } }, res: { send(body: string): void }): void {
  res.send(req.params.id);
}

const handlers: Handler[] = [authenticate, lookup];

export const router = Router();
router.get(LOOKUP_ROUTE, ...handlers);
