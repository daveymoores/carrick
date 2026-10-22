// 5. Constructed inside a function that passes its own parameter: the base
// is whatever that function's callers pass, which no text here states.
export class Reports {
  constructor(private base: string) {}

  all(): Promise<Response> {
    return fetch(`${this.base}/reports`);
  }
}

export function reportsAt(base: string): Reports {
  return new Reports(base);
}
