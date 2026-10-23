// 8. Two classes, each constructing the other with its own field: `Ping`'s
// `send` is what `Pong` passes, which is `Pong`'s `send`, which is what
// `Ping` passes. Each is left with its `??` right side, `fetch`.
export interface SendOptions {
  send?: typeof fetch;
}

export class Ping {
  private readonly send: typeof fetch;

  constructor(options: SendOptions = {}) {
    this.send = options.send ?? fetch;
  }

  pong(): Pong {
    return new Pong({ send: this.send });
  }

  go(): Promise<Response> {
    return this.send("/api/ping");
  }
}

export class Pong {
  private readonly send: typeof fetch;

  constructor(options: SendOptions = {}) {
    this.send = options.send ?? fetch;
  }

  ping(): Ping {
    return new Ping({ send: this.send });
  }

  go(): Promise<Response> {
    return this.send("/api/pong");
  }
}
