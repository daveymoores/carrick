// 7. A method constructs its own class, passing its own field on. The
// field is what the one construction passes, which is the field itself:
// reading it adds nothing, and the `??` right side, `fetch`, is left.
export interface ForkOptions {
  send?: typeof fetch;
}

export class Fork {
  private readonly send: typeof fetch;

  constructor(options: ForkOptions = {}) {
    this.send = options.send ?? fetch;
  }

  child(): Fork {
    return new Fork({ send: this.send });
  }

  get(): Promise<Response> {
    return this.send("/api/fork");
  }
}
