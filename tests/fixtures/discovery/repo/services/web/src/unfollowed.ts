import { Socket, describe } from "fixture-socket";
import { ping } from "fixture-plain";
import { listShared } from "@fixture/shared";

export function outside(): Promise<Response> {
  return listShared();
}

export function unmatched(socket: Socket): string {
  return describe(socket);
}

export function unanswered(): void {
  ping("https://plain.example");
}

function double(n: number): number {
  return n * 2;
}

export function pure(): number {
  return Math.max(double(2), 1);
}
