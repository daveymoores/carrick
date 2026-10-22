import { requireUser } from "./session";

export function ordersLoader(request: Request): string {
  const user = requireUser(request);
  return user ?? "guest";
}

export function profileLoader(request: Request): string {
  const user = requireUser(request);
  return user ?? "guest";
}

export function billingLoader(request: Request): string {
  const user = requireUser(request);
  return user ?? "guest";
}
