// 10. Constructed only by its own static factory, which another module
// calls through the class (`Hosts.local()`): reading a member of the class
// is not handing the class on, so the field reads as the factory's literal.
export class Hosts {
  constructor(private base: string) {}

  static local(): Hosts {
    return new Hosts("/local");
  }

  list(): Promise<Response> {
    return fetch(`${this.base}/hosts`);
  }
}
