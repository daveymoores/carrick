async function send(path: string, method: string): Promise<unknown> {
  const reply = await fetch(path, { method });
  return reply.json();
}

export async function placeOrder(): Promise<unknown> {
  return send("/api/orders", "POST");
}
