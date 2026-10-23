// 7. A parameter no caller fills and no default gives.
export function withSender(send: (path: string) => Promise<Response>): Promise<Response> {
  return send("/api/never");
}
