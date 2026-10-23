// 1. A parameter property, constructed once in another module.
export class Api {
  constructor(private base: string) {}

  users(): Promise<Response> {
    return fetch(`${this.base}/users`);
  }
}
