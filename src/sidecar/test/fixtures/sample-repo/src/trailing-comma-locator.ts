// #335: multi-line object literal with trailing commas, matched by an LLM
// text locator that prints the same expression single-line WITHOUT the
// trailing comma. `normalizeWhitespace` only collapsed whitespace, so the
// comma defeated exact AND containment matching and the reverse-substring
// fallback bound the locator to the smallest embedded fragment ("active").
interface StatusPayload {
  service: string;
  status: string;
  timestamp: string;
  userCount: number;
}

interface StatusResLike {
  json: (data: StatusPayload) => void;
}

export function sendStatus(res: StatusResLike, userCount: number) {
  res.json({
    service: "notification-service",
    status: "active",
    timestamp: new Date().toISOString(),
    userCount: userCount,
  });
}
