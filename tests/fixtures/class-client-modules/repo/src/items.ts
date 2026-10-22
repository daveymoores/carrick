// 2. A request function set from a constructor option, with the global
// `fetch` as the fallback, and a base from another option.
export interface ItemsOptions {
  base: string;
  send?: typeof fetch;
}

export class Items {
  private readonly send: typeof fetch;
  private readonly base: string;

  constructor(opts: ItemsOptions) {
    this.send = opts.send ?? fetch;
    this.base = opts.base;
  }

  list(): Promise<Response> {
    return this.send(this.base + "/items");
  }
}
