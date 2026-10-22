// 6. A request function set from a constructor option defaulting to an
// empty object, constructed inside a factory that passes its own parameter
// on. Nothing calls the factory, so the option is its default `{}`, which
// holds no `send`: the client sends with `fetch`.
export interface JobsOptions {
  send?: typeof fetch;
}

export class Jobs {
  private readonly send: typeof fetch;

  constructor(options: JobsOptions = {}) {
    this.send = options.send ?? fetch;
  }

  run(): Promise<Response> {
    return this.send("/api/jobs", { method: "POST" });
  }
}

export function jobsFrom(options: JobsOptions = {}): Jobs {
  return new Jobs(options);
}
