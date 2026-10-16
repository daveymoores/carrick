const OPS_URL = `${process.env.OPS_API_URL}/v1/ops`;

// The operation is the caller's, and so are the rest of the keys: the body
// writes `op` before it spreads the caller's object over it.
export async function sendOp(op: string, args?: object): Promise<unknown> {
  const response = await fetch(OPS_URL, {
    method: "POST",
    body: JSON.stringify({ op, ...args }),
  });
  return response.json();
}
