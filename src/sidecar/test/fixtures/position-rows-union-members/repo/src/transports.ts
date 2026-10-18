export class HttpTransport {
  get(path: string): Promise<string> {
    return Promise.resolve(path);
  }
}

export class CachedTransport {
  get(path: string): Promise<string> {
    return Promise.resolve(path);
  }
}

export class Box<T> {
  constructor(private readonly value: T) {}
  read(): T {
    return this.value;
  }
}

export class Overloaded {
  get(path: string): Promise<string>;
  get(path: string, retries: number): Promise<string>;
  get(path: string): Promise<string> {
    return Promise.resolve(path);
  }
}

export type Verb = "get" | "post";
export type MappedClient = { [K in Verb]: (path: string) => Promise<unknown> };
