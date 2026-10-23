// 9. Constructed once with a literal, and handed on as a value in another
// module: whatever receives the class may build it with any base.
export class Hooks {
  constructor(private base: string) {}

  hook(): Promise<Response> {
    return fetch(`${this.base}/hook`);
  }
}
