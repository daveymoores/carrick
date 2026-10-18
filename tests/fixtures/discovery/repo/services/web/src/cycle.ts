export function ping(n: number): void {
  if (n > 0) pong(n - 1);
}

export function pong(n: number): void {
  if (n > 0) ping(n - 1);
}
