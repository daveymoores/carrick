const JOBS_URL = `${process.env.JOBS_API_URL}/v1/jobs`;

let sequence = 0;

// Two spreads of parameters: which one sets `op` is not one rule.
export async function sendTwice(op: string, head: object, tail: object): Promise<void> {
  await fetch(JOBS_URL, {
    method: "POST",
    body: JSON.stringify({ op, ...head, ...tail }),
  });
}

// A spread of one key of a parameter.
export async function sendExtra(op: string, options: { extra: object }): Promise<void> {
  await fetch(JOBS_URL, {
    method: "POST",
    body: JSON.stringify({ op, ...options.extra }),
  });
}

// The parameter is assigned again before it is spread.
export async function sendCopied(op: string, given: object): Promise<void> {
  given = { ...given, copied: true };
  await fetch(JOBS_URL, {
    method: "POST",
    body: JSON.stringify({ op, ...given }),
  });
}

// The parameter is written through before it is spread.
export async function sendStamped(op: string, stamped: { stamp?: number }): Promise<void> {
  sequence += 1;
  stamped.stamp = sequence;
  await fetch(JOBS_URL, {
    method: "POST",
    body: JSON.stringify({ op, ...stamped }),
  });
}
